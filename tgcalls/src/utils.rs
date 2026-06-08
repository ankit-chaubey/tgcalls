use std::ffi::{CStr, CString};

pub trait IntoCString {
    fn into_c_string(self) -> CString;
}

impl IntoCString for String {
    fn into_c_string(self) -> CString {
        truncate_at_nul(self.into_bytes())
    }
}

impl IntoCString for &str {
    fn into_c_string(self) -> CString {
        truncate_at_nul(self.as_bytes().to_vec())
    }
}

impl IntoCString for CString {
    fn into_c_string(self) -> CString {
        self
    }
}

impl IntoCString for &CStr {
    fn into_c_string(self) -> CString {
        self.to_owned()
    }
}

fn truncate_at_nul(mut bytes: Vec<u8>) -> CString {
    if let Some(pos) = bytes.iter().position(|&b| b == 0) {
        bytes.truncate(pos);
    }
    CString::new(bytes).unwrap()
}
