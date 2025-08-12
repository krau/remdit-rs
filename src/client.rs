use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use std::path::PathBuf;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::config::Server;

// Simple random number generator
fn simple_random() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(12345);
    // Simple hash to make it more random
    nanos.wrapping_mul(1103515245).wrapping_add(12345)
}

// Simple JSON parser for SessionResponse
fn parse_session_response(json: &str) -> Result<SessionResponse> {
    let mut sessionid = String::new();
    let mut editurl = String::new();

    // Handle both single-line and multi-line JSON
    let json = json.replace('\n', " ").replace('\r', " ");

    // Find sessionid
    if let Some(start) = json.find("\"sessionid\"") {
        if let Some(colon_pos) = json[start..].find(':') {
            let value_start = start + colon_pos + 1;
            if let Some(value) = extract_json_value_from_position(&json, value_start) {
                sessionid = value;
            }
        }
    }

    // Find editurl
    if let Some(start) = json.find("\"editurl\"") {
        if let Some(colon_pos) = json[start..].find(':') {
            let value_start = start + colon_pos + 1;
            if let Some(value) = extract_json_value_from_position(&json, value_start) {
                editurl = value;
            }
        }
    }

    if sessionid.is_empty() || editurl.is_empty() {
        return Err(anyhow!("Invalid session response format. JSON: {}", json));
    }

    Ok(SessionResponse { sessionid, editurl })
}

// Simple JSON parser for WebSocket messages
fn parse_websocket_message(json: &str) -> Result<WebSocketMessage> {
    let mut msg_type = String::new();
    let mut content: Option<String> = None;

    // Handle both single-line and multi-line JSON
    let json = json.replace('\n', " ").replace('\r', " ");

    // Find type
    if let Some(start) = json.find("\"type\"") {
        if let Some(colon_pos) = json[start..].find(':') {
            let value_start = start + colon_pos + 1;
            if let Some(value) = extract_json_value_from_position(&json, value_start) {
                msg_type = value;
            }
        }
    }

    // Find content
    if let Some(start) = json.find("\"content\"") {
        if let Some(colon_pos) = json[start..].find(':') {
            let value_start = start + colon_pos + 1;
            if let Some(value) = extract_json_value_from_position(&json, value_start) {
                content = Some(value);
            }
        }
    }

    Ok(WebSocketMessage { msg_type, content })
}

// Simple JSON serializer for ResultMessage
fn serialize_result_message(msg: &ResultMessage) -> String {
    let reason = match &msg.reason {
        Some(r) => format!(",\"reason\":\"{}\"", escape_json_string(r)),
        None => String::new(),
    };

    format!(
        "{{\"type\":\"{}\",\"success\":{}{}}}",
        escape_json_string(&msg.msg_type),
        msg.success,
        reason
    )
}

// Extract JSON value from a specific position in the string
fn extract_json_value_from_position(json: &str, start_pos: usize) -> Option<String> {
    let remaining = &json[start_pos..].trim_start();

    if remaining.starts_with('"') {
        // String value
        let mut end_pos = 1;
        let chars: Vec<char> = remaining.chars().collect();
        while end_pos < chars.len() {
            if chars[end_pos] == '"' && (end_pos == 1 || chars[end_pos - 1] != '\\') {
                let value = chars[1..end_pos].iter().collect::<String>();
                return Some(unescape_json_string(&value));
            }
            end_pos += 1;
        }
    }

    None
}

// Simple JSON string escaping
fn escape_json_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

// Simple JSON string unescaping
fn unescape_json_string(s: &str) -> String {
    s.replace("\\\"", "\"")
        .replace("\\\\", "\\")
        .replace("\\n", "\n")
        .replace("\\r", "\r")
        .replace("\\t", "\t")
}

#[derive(Debug)]
struct SessionResponse {
    sessionid: String,
    editurl: String,
}

#[derive(Debug)]
struct WebSocketMessage {
    msg_type: String,
    content: Option<String>,
}

#[derive(Debug)]
struct ResultMessage {
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

        let boundary = format!("----WebKitFormBoundary{}", simple_random());
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

        let response = minreq::post(&url)
            .with_header(
                "Content-Type",
                &format!("multipart/form-data; boundary={}", boundary),
            )
            .with_header("X-API-Key", self.server.key.as_deref().unwrap_or(""))
            .with_body(&body[..])
            .send();

        let response = match response {
            Ok(resp) => {
                if resp.status_code == 401 {
                    return Err(anyhow!("Unauthorized: check your API key"));
                }
                if resp.status_code < 200 || resp.status_code >= 300 {
                    return Err(anyhow!("Request failed with status: {}", resp.status_code));
                }
                resp
            }
            Err(e) => return Err(anyhow!("Request failed: {}", e)),
        };

        let content_type = response
            .headers
            .get("content-type")
            .map(|v| v.as_str())
            .unwrap_or("");
        if !content_type.contains("application/json") {
            return Err(anyhow!("Unexpected content-type: {}", content_type));
        }

        let session_resp = parse_session_response(&response.as_str()?)?;
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
                    let ws_msg = parse_websocket_message(&text)?;

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
                                        let json = serialize_result_message(&result_msg);
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
                                        let json = serialize_result_message(&result_msg);
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
