pub const MAX_IMAGE_BYTES: usize = 15 << 20;

pub fn check_png(png: &[u8]) -> Result<(), String> {
    if png.len() > MAX_IMAGE_BYTES {
        return Err("the image is larger than 15 MiB; nothing was pasted".into());
    }
    if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("the clipboard image is not PNG data; nothing was pasted".into());
    }
    Ok(())
}

pub fn quoted_image_path(path: &str) -> Result<String, String> {
    if path.is_empty() || path.chars().any(char::is_control) {
        return Err("the image cache path contains a control character; nothing was pasted".into());
    }
    let mut quoted = String::with_capacity(path.len() + 2);
    quoted.push('"');
    for character in path.chars() {
        if matches!(character, '\\' | '"' | '$' | '`') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('"');
    Ok(quoted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_paths_are_double_quoted_without_shell_expansion() {
        assert_eq!(
            quoted_image_path("/home/o'neil/a $b`c\"d\\e.png").unwrap(),
            "\"/home/o'neil/a \\$b\\`c\\\"d\\\\e.png\""
        );
        assert!(quoted_image_path("/tmp/a\nb.png").is_err());
    }

    #[test]
    fn invalid_or_oversized_image_data_is_refused() {
        assert!(check_png(&[]).is_err());
        assert!(check_png(b"a filename.png").is_err());
        let mut large = b"\x89PNG\r\n\x1a\n".to_vec();
        large.resize(MAX_IMAGE_BYTES + 1, 0);
        assert!(check_png(&large).unwrap_err().contains("15 MiB"));
    }
}
