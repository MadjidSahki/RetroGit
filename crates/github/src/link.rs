/// Extract the `rel="next"` URL from a GitHub `Link` header.
pub fn next_link(header: &str) -> Option<String> {
    header.split(',').find_map(|part| {
        let mut pieces = part.split(';');
        let url = pieces.next()?.trim();
        let is_next = pieces.any(|p| p.trim() == r#"rel="next""#);
        if is_next && url.starts_with('<') && url.ends_with('>') {
            Some(url[1..url.len() - 1].to_string())
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::next_link;

    #[test]
    fn finds_next_among_several_rels() {
        let h = r#"<https://api.github.com/user/repos?page=2>; rel="next", <https://api.github.com/user/repos?page=5>; rel="last""#;
        assert_eq!(
            next_link(h).as_deref(),
            Some("https://api.github.com/user/repos?page=2")
        );
    }

    #[test]
    fn next_can_be_last_in_the_list() {
        let h = r#"<https://x/?page=1>; rel="prev", <https://x/?page=3>; rel="next""#;
        assert_eq!(next_link(h).as_deref(), Some("https://x/?page=3"));
    }

    #[test]
    fn none_on_last_page_or_garbage() {
        assert_eq!(next_link(r#"<https://x/?page=1>; rel="prev""#), None);
        assert_eq!(next_link(""), None);
        assert_eq!(next_link("garbage"), None);
    }
}
