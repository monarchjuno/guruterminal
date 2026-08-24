pub(crate) fn valid_json_pointer(value: &str, max_bytes: usize, allow_root: bool) -> bool {
    if value.len() > max_bytes || value.contains(['\0', '\n', '\r']) {
        return false;
    }
    if value.is_empty() {
        return allow_root;
    }
    if !value.starts_with('/') {
        return false;
    }

    let bytes = value.as_bytes();
    let mut index = 1;
    while index < bytes.len() {
        if bytes[index] != b'~' {
            index += 1;
            continue;
        }
        if index + 1 >= bytes.len() || !matches!(bytes[index + 1], b'0' | b'1') {
            return false;
        }
        index += 2;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_6901_escapes_are_strict_and_root_is_policy_controlled() {
        for pointer in ["/plain", "/a~0b", "/a~1b", "/", "//", "/~01"] {
            assert!(valid_json_pointer(pointer, 512, true), "{pointer}");
        }
        for pointer in ["plain", "/bad~2", "/trailing~", "/bad\nkey"] {
            assert!(!valid_json_pointer(pointer, 512, true), "{pointer}");
        }
        assert!(valid_json_pointer("", 512, true));
        assert!(!valid_json_pointer("", 512, false));
    }
}
