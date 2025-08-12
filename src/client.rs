use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::config::Server;

#[derive(Debug, Deserialize)]
struct SessionResponse {
    sessionid: String,
    editurl: String,
}

#[derive(Debug, Deserialize)]
struct WebSocketMessage {
    #[serde(rename = "type")]
    msg_type: String,
    content: Option<String>,
}

#[derive(Debug, Serialize)]
struct ResultMessage {
    #[serde(rename = "type")]
    msg_type: String,
    success: bool,
    reason: Option<String>,
}

pub struct Client {
    pub server: Server,
    pub file_path: PathBuf,
    session_id: Option<String>,
    edit_url: Option<String>,
    ws_stream: Option<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
}

impl Client {
    pub fn new(server: Server, file_path: PathBuf) -> Result<Self> {
        Ok(Self {
            server,
            file_path,
            session_id: None,
            edit_url: None,
            ws_stream: None,
        })
    }

    pub async fn create_session(&mut self) -> Result<()> {
        let mut server_url = self.server.addr.clone();

        if !server_url.starts_with("http://") && !server_url.starts_with("https://") {
            server_url = format!("https://{}", server_url);
        }

        let url = format!("{}/api/session", server_url);

        let file_content = tokio::fs::read(&self.file_path).await?;
        let file_name = self
            .file_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow!("Invalid file name"))?;

        let boundary = format!("----WebKitFormBoundary{}", fastrand::u64(..));
        let mut body = Vec::new();

        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"document\"; filename=\"{}\"\r\n",
                file_name
            )
            .as_bytes(),
        );
        body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
        body.extend_from_slice(&file_content);
        body.extend_from_slice(format!("\r\n--{}--\r\n", boundary).as_bytes());

        let mut request = ureq::post(&url).header(
            "Content-Type",
            &format!("multipart/form-data; boundary={}", boundary),
        );

        // Add API key if present
        if let Some(ref key) = self.server.key {
            request = request.header("X-API-Key", key);
        }

        let response = request.send(&body[..]);

        let response = match response {
            Ok(resp) => resp,
            Err(ureq::Error::StatusCode(401)) => {
                return Err(anyhow!("Unauthorized: check your API key"));
            }
            Err(e) => return Err(anyhow!("Request failed: {}", e)),
        };

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !content_type.contains("application/json") {
            return Err(anyhow!("Unexpected content-type: {}", content_type));
        }

        let session_resp: SessionResponse = response.into_body().read_json()?;
        self.session_id = Some(session_resp.sessionid);
        self.edit_url = Some(session_resp.editurl);

        Ok(())
    }

    pub fn get_edit_url(&self) -> String {
        self.edit_url.as_ref().cloned().unwrap_or_default()
    }

    pub async fn connect(&mut self) -> Result<()> {
        let session_id = self
            .session_id
            .as_ref()
            .ok_or_else(|| anyhow!("No session ID available"))?;

        let mut server_url = self.server.addr.clone();

        // Convert HTTP(S) to WS(S)
        if server_url.starts_with("https://") {
            server_url = server_url.replace("https://", "wss://");
        } else if server_url.starts_with("http://") {
            server_url = server_url.replace("http://", "ws://");
        } else {
            server_url = format!("wss://{}", server_url);
        }

        let ws_url = format!("{}/api/session/{}", server_url, session_id);

        let (ws_stream, _) = connect_async(&ws_url).await?;
        self.ws_stream = Some(ws_stream);

        Ok(())
    }

    pub async fn handle_messages(&mut self) -> Result<()> {
        while let Some(ws_stream) = &mut self.ws_stream {
            let msg = match ws_stream.next().await {
                Some(msg) => msg?,
                _ => break,
            };

            match msg {
                Message::Text(text) => {
                    let ws_msg: WebSocketMessage = serde_json::from_str(&text)?;

                    match ws_msg.msg_type.as_str() {
                        "save" => {
                            if let Some(content) = ws_msg.content {
                                match tokio::fs::write(&self.file_path, &content).await {
                                    Ok(_) => {
                                        eprintln!("File saved with {} bytes", content.len());
                                        // Send result message
                                        let result_msg = ResultMessage {
                                            msg_type: "save_result".to_string(),
                                            success: true,
                                            reason: Some("File saved successfully".to_string()),
                                        };
                                        let json = serde_json::to_string(&result_msg)?;
                                        if let Err(_e) = ws_stream.send(Message::Text(json)).await {
                                            eprintln!("Failed to send result message");
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("Failed to write file: {}", e);
                                        // Send result message
                                        let result_msg = ResultMessage {
                                            msg_type: "save_result".to_string(),
                                            success: false,
                                            reason: Some("Failed to save file".to_string()),
                                        };
                                        let json = serde_json::to_string(&result_msg)?;
                                        if let Err(_e) = ws_stream.send(Message::Text(json)).await {
                                            eprintln!("Failed to send result message");
                                        }
                                    }
                                }
                            }
                        }
                        _ => {
                            eprintln!("Unknown message type: {}", ws_msg.msg_type);
                        }
                    }
                }
                Message::Close(_) => {
                    eprintln!("WebSocket connection closed");
                    break;
                }
                Message::Ping(_) => {}
                _ => {}
            }
        }

        Ok(())
    }

    pub async fn close(&mut self, code: u16, reason: &str) -> Result<()> {
        if let Some(ws_stream) = &mut self.ws_stream {
            let close_frame = tungstenite::protocol::CloseFrame {
                code: tungstenite::protocol::frame::coding::CloseCode::from(code),
                reason: reason.to_string().into(),
            };
            ws_stream.send(Message::Close(Some(close_frame))).await?;
        }
        Ok(())
    }
}
