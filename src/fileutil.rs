use std::path::Path;

pub fn is_exist<P: AsRef<Path>>(path: P) -> bool {
    Path::new(path.as_ref()).exists()
}

pub fn is_dir<P: AsRef<Path>>(path: P) -> bool {
    Path::new(path.as_ref()).is_dir()
}
