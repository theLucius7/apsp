use super::*;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

mod transport_tests;

/// The Rust sources below `crates/`, without build output.
fn sources(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("readable source directory") {
        let entry = entry.expect("directory entry");
        let kind = entry.file_type().expect("file type");
        let path = entry.path();
        if kind.is_dir() && entry.file_name() != "target" {
            sources(&path, found);
        } else if kind.is_file() && path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

/// Each vendor API is reached through its one client here: the API's address appears in no
/// other source, so a second client, or a test that would reach the real service, fails.
#[test]
fn vendor_api_addresses_appear_only_in_their_clients() {
    // Spelled in two parts so that this guard does not name them itself.
    let clients = [
        (
            concat!("api.cloud", "flare.com"),
            "cloud-api/src/cloudflare.rs",
        ),
        (
            concat!("api.global", "ping.io"),
            "cloud-api/src/globalping.rs",
        ),
    ];
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates directory");
    let mut files = Vec::new();
    sources(crates, &mut files);
    assert!(files.len() > 100, "the workspace sources were not found");
    let mut misplaced = Vec::new();
    for path in &files {
        let relative = path.strip_prefix(crates).expect("path under crates/");
        let source = fs::read_to_string(path).expect("readable source");
        for (address, client) in clients {
            if relative == Path::new(client) {
                continue;
            }
            for (index, line) in source.lines().enumerate() {
                if line.contains(address) {
                    misplaced.push(format!("{}:{}: {}", path.display(), index + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        misplaced.is_empty(),
        "call these vendors through their clients in crates/cloud-api:\n{}",
        misplaced.join("\n")
    );
    for (address, client) in clients {
        let source = fs::read_to_string(crates.join(client)).expect("client source");
        assert!(
            source.contains(address),
            "{client} no longer names {address}"
        );
    }
}

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
