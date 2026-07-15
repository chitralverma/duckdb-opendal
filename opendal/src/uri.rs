use std::ffi::{c_char, CString};

use percent_encoding::percent_decode_str;

use crate::error::OdErrorCode;
use crate::ffi::cstr;

pub(crate) unsafe fn resolve(
    url: *const c_char,
) -> Result<(CString, CString, CString), (OdErrorCode, String)> {
    let url = cstr(url).ok_or((
        OdErrorCode::InvalidInput,
        "url is null or not UTF-8".to_string(),
    ))?;
    if url.contains('?') || url.contains('#') {
        return Err((
            OdErrorCode::InvalidInput,
            "query strings and fragments are not supported in OpenDAL file URLs".to_string(),
        ));
    }
    let (scheme, remainder) = url.split_once("://").ok_or((
        OdErrorCode::InvalidInput,
        "url must use scheme://authority/path syntax".to_string(),
    ))?;
    let scheme = scheme.to_ascii_lowercase();
    opendal::init_default_registry();
    if !opendal::OperatorRegistry::get().schemes().contains(&scheme) {
        return Err((
            OdErrorCode::Unsupported,
            format!("scheme '{scheme}' is not registered"),
        ));
    }
    let (authority, encoded_path) = remainder.split_once('/').unwrap_or((remainder, ""));
    if authority.contains('@') {
        return Err((
            OdErrorCode::InvalidInput,
            "userinfo is not supported in OpenDAL file URLs".to_string(),
        ));
    }
    if encoded_path.to_ascii_lowercase().contains("%2f") {
        return Err((
            OdErrorCode::InvalidInput,
            "percent-encoded path separators are not supported".to_string(),
        ));
    }
    let path = percent_decode_str(encoded_path)
        .decode_utf8()
        .map_err(|_| {
            (
                OdErrorCode::InvalidInput,
                "operation path is not valid UTF-8".to_string(),
            )
        })?
        .into_owned();
    if path.contains('\0') {
        return Err((
            OdErrorCode::InvalidInput,
            "operation path contains a NUL byte".to_string(),
        ));
    }
    Ok((
        CString::new(scheme).unwrap(),
        CString::new(authority).unwrap(),
        CString::new(path).unwrap(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(url: &str) -> (String, String, String) {
        opendal::init_default_registry();
        let url = CString::new(url).unwrap();
        let (scheme, authority, path) = match unsafe { resolve(url.as_ptr()) } {
            Ok(parts) => parts,
            Err((_, message)) => panic!("resolve failed: {message}"),
        };
        (
            scheme.into_string().unwrap(),
            authority.into_string().unwrap(),
            path.into_string().unwrap(),
        )
    }

    #[test]
    fn resolves_universal_url_shape() {
        assert_eq!(
            parsed("S3://bucket/a//b%20c/"),
            (
                "s3".to_string(),
                "bucket".to_string(),
                "a//b c/".to_string()
            )
        );
        assert_eq!(
            parsed("memory:///registry/path"),
            (
                "memory".to_string(),
                "".to_string(),
                "registry/path".to_string()
            )
        );
    }

    #[test]
    fn rejects_ambiguous_url_features() {
        for url in [
            "s3://bucket/key?version=1",
            "s3://bucket/key#fragment",
            "s3://user@bucket/key",
            "s3://bucket/a%2Fb",
        ] {
            let url = CString::new(url).unwrap();
            assert!(unsafe { resolve(url.as_ptr()) }.is_err());
        }
    }
}
