use super::*;
use std::collections::BTreeMap;

mod transport_tests;

#[test]
fn huawei_signing_retains_and_sorts_all_encoded_query_pairs() {
    let url = reqwest::Url::parse("https://dns.myhuaweicloud.com/v2/zones/00000000000000000000000000000001/recordsets?name=node.example.com.&name=*.example.com.&empty&name=*.example.com.&q=+&q=%2B").unwrap();
    assert_eq!(
        signing::huawei_query(&url),
        "empty=&name=%2A.example.com.&name=%2A.example.com.&name=node.example.com.&q=%20&q=%2B"
    );
    assert_eq!(
        signing::huawei_query(&reqwest::Url::parse("https://dns.myhuaweicloud.com/").unwrap()),
        ""
    );
}

#[test]
fn independent_rfc3986_and_hmac_vectors_cover_unicode_and_all_provider_schemes() {
    assert_eq!(
        signing::encode("节点 *.example.com/+~"),
        "%E8%8A%82%E7%82%B9%20%2A.example.com%2F%2B~"
    );
    let parameters: BTreeMap<String, String> = [
        ("Action", "DescribeDomainInfo"),
        ("AccessKeyId", "TEST_ONLY_ID"),
        ("DomainName", "*.example.com"),
        ("SignatureNonce", "test nonce+/?~"),
        ("Timestamp", "2026-10-01T00:00:00Z"),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into()))
    .collect();
    assert_eq!(
        signing::aliyun("TEST_ONLY_SECRET", &parameters),
        "+9OGkoeAQj1Et20Wv5YNUn4VkvE="
    );
    assert_eq!(signing::iso_time(1790812800), "2026-10-01T00:00:00Z");
    assert!(
        signing::tencent(
            "TEST_ONLY_ID",
            "TEST_ONLY_SECRET",
            "dnspod.tencentcloudapi.com",
            "DescribeDomain",
            r#"{"Domain":"example.com"}"#,
            1790812800
        )
        .ends_with("Signature=475c02c69fd979bd33221eb24302996c74f13de2418313ab24f2db11b9662513")
    );
    let url=reqwest::Url::parse("https://dns.myhuaweicloud.com/v2/zones/00000000000000000000000000000001/recordsets?search_mode=equal&name=*.example.com.").unwrap();
    assert!(
        signing::huawei(
            "TEST_ONLY_ID",
            "TEST_ONLY_SECRET",
            "GET",
            &url,
            "",
            "20261001T000000Z"
        )
        .ends_with("Signature=07790cdd5824700026ef982c6b77ae887193edadd37dbff866314224af6598bb")
    );
}
