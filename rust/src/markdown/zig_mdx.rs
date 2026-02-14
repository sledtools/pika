#[derive(Debug, Clone)]
pub enum ParseError {
    #[cfg(zig_mdx_disabled)]
    Disabled,
    AbiMismatch,
    InputTooLarge,
    ParseFailed,
    InvalidUtf8,
    AllocFailed,
    InternalFailed,
    NullOutput,
    OutputUtf8,
    UnknownStatus,
}

#[cfg(zig_mdx_disabled)]
pub fn parse_markdown_json(_input: &str) -> Result<String, ParseError> {
    Err(ParseError::Disabled)
}

#[cfg(not(zig_mdx_disabled))]
mod enabled {
    use super::ParseError;
    use std::slice;
    use std::sync::OnceLock;

    const STATUS_OK: i32 = 0;
    const STATUS_PARSE_ERROR: i32 = 1;
    const STATUS_INVALID_UTF8: i32 = 2;
    const STATUS_ALLOC_ERROR: i32 = 3;
    const STATUS_INTERNAL_ERROR: i32 = 4;

    const ABI_VERSION: u32 = 1;
    const MAX_INPUT_BYTES: usize = 32 * 1024;

    static ABI_CHECK: OnceLock<Result<(), ParseError>> = OnceLock::new();

    unsafe extern "C" {
        fn zigmdx_parse_json(
            input_ptr: *const u8,
            input_len: usize,
            out_json_ptr: *mut *mut u8,
            out_json_len: *mut usize,
        ) -> i32;

        fn zigmdx_free_json(ptr: *mut u8, len: usize);

        fn zigmdx_abi_version() -> u32;
    }

    fn ensure_abi() -> Result<(), ParseError> {
        ABI_CHECK
            .get_or_init(|| {
                let got = unsafe { zigmdx_abi_version() };
                if got == ABI_VERSION {
                    Ok(())
                } else {
                    Err(ParseError::AbiMismatch)
                }
            })
            .clone()
    }

    pub fn parse_markdown_json(input: &str) -> Result<String, ParseError> {
        ensure_abi()?;

        if input.len() > MAX_INPUT_BYTES {
            return Err(ParseError::InputTooLarge);
        }

        let mut out_ptr: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;

        let status = unsafe {
            zigmdx_parse_json(
                input.as_ptr(),
                input.len(),
                &mut out_ptr as *mut *mut u8,
                &mut out_len as *mut usize,
            )
        };

        match status {
            STATUS_OK => {}
            STATUS_PARSE_ERROR => return Err(ParseError::ParseFailed),
            STATUS_INVALID_UTF8 => return Err(ParseError::InvalidUtf8),
            STATUS_ALLOC_ERROR => return Err(ParseError::AllocFailed),
            STATUS_INTERNAL_ERROR => return Err(ParseError::InternalFailed),
            _ => return Err(ParseError::UnknownStatus),
        }

        if out_len > 0 && out_ptr.is_null() {
            return Err(ParseError::NullOutput);
        }

        let bytes = unsafe { slice::from_raw_parts(out_ptr, out_len) };
        let json = String::from_utf8(bytes.to_vec()).map_err(|_| ParseError::OutputUtf8)?;
        unsafe {
            zigmdx_free_json(out_ptr, out_len);
        }
        Ok(json)
    }
}

#[cfg(not(zig_mdx_disabled))]
pub use enabled::parse_markdown_json;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_too_large_is_rejected() {
        let input = "a".repeat(40 * 1024);
        let res = parse_markdown_json(&input);

        #[cfg(zig_mdx_disabled)]
        assert!(matches!(res, Err(ParseError::Disabled)));

        #[cfg(not(zig_mdx_disabled))]
        assert!(matches!(res, Err(ParseError::InputTooLarge)));
    }

    #[test]
    fn parse_simple_heading_returns_root_json() {
        let res = parse_markdown_json("# hi");

        #[cfg(zig_mdx_disabled)]
        assert!(matches!(res, Err(ParseError::Disabled)));

        #[cfg(not(zig_mdx_disabled))]
        match res {
            Ok(json) => {
                assert!(json.contains("\"type\":\"root\""));
                assert!(json.contains("\"heading\""));
            }
            Err(err) => panic!("unexpected parse error: {err:?}"),
        }
    }
}
