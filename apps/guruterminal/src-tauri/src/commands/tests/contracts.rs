use super::*;

#[test]
fn external_links_allow_only_credential_free_web_urls() {
    assert_eq!(
        validated_external_url("https://dart.fss.or.kr/dsab001/search.ax?textCrpNm=005930")
            .unwrap()
            .scheme(),
        "https",
    );
    assert!(validated_external_url("http://example.com/report").is_ok());
    assert!(validated_external_url("file:///tmp/private").is_err());
    assert!(validated_external_url("javascript:alert(1)").is_err());
    assert!(validated_external_url("https://user:secret@example.com").is_err());
    assert!(validated_external_url("not a URL").is_err());
    assert!(
        validated_external_url(&format!("https://example.com/{}", "a".repeat(8 * 1024))).is_err()
    );
}

#[test]
fn chat_fallback_title_is_local_and_deterministic() {
    assert_eq!(fallback_chat_title("삼성전자\n두 번째 질문"), "삼성전자");
}
