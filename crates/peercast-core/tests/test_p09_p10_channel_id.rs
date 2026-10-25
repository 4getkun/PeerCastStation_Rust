use peercast_core::channel_id::{derive_broadcast_channel_id, ChannelId, ChannelNetworkType};

fn parse_guid_with_hyphens(s: &str) -> ChannelId {
    let clean: String = s.chars().filter(|c| *c != '-').collect();
    ChannelId::from_hex(&clean).unwrap()
}

#[test]
fn test_channel_id_dotnet_guid_layout_conversion() {
    // Non-symmetric GUID
    let canonical_hex = "00112233445566778899aabbccddeeff";
    let cid = ChannelId::from_hex(canonical_hex).unwrap();

    // In .NET Guid layout:
    // bytes 0..4 (00 11 22 33) reversed -> 33 22 11 00
    // bytes 4..6 (44 55) reversed -> 55 44
    // bytes 6..8 (66 77) reversed -> 77 66
    // bytes 8..16 (88 99 aa bb cc dd ee ff) unchanged
    let dotnet_bytes = cid.to_dotnet_guid_bytes();
    let expected_dotnet: [u8; 16] = [
        0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff,
    ];
    assert_eq!(dotnet_bytes, expected_dotnet);

    // Round-trip back to canonical ChannelId
    let reconstructed = ChannelId::from_dotnet_guid_bytes(dotnet_bytes);
    assert_eq!(reconstructed, cid);
    assert_eq!(reconstructed.to_hex_lower(), canonical_hex);
}

#[test]
fn test_station_broadcast_id_derivation_reference_vectors() {
    let bcid = parse_guid_with_hyphens("00112233-4455-6677-8899-aabbccddeeff");

    // Vector 1: 0-empty
    let id_0_empty = derive_broadcast_channel_id(&bcid, ChannelNetworkType::IPv4, "", "", "");
    assert_eq!(
        id_0_empty.to_hex_lower(),
        "8852c02562a3d36cd274ea3c5ae23d7d"
    );

    // Vector 2: 0-japanese
    let id_0_jp = derive_broadcast_channel_id(
        &bcid,
        ChannelNetworkType::IPv4,
        "深夜の放送",
        "音楽",
        "http://localhost:8080/",
    );
    assert_eq!(id_0_jp.to_hex_lower(), "697d1c4d1ab44fdc9269d4cdcfdc5967");

    // Vector 3: 0-length_boundary
    let name_128 = "x".repeat(128);
    let genre_127 = "x".repeat(127);
    let source_jp = "あ".repeat(43);
    let id_0_boundary = derive_broadcast_channel_id(
        &bcid,
        ChannelNetworkType::IPv4,
        &name_128,
        &genre_127,
        &source_jp,
    );
    assert_eq!(
        id_0_boundary.to_hex_lower(),
        "1ee8bb3017038c02e7522508e831ac09"
    );

    // Vector 4: 1-empty (IPv6 network)
    let id_1_empty = derive_broadcast_channel_id(&bcid, ChannelNetworkType::IPv6, "", "", "");
    assert_eq!(
        id_1_empty.to_hex_lower(),
        "010dbe652e323b6360297cdd22ea5453"
    );

    // Vector 5: 1-japanese (IPv6 network)
    let id_1_jp = derive_broadcast_channel_id(
        &bcid,
        ChannelNetworkType::IPv6,
        "深夜の放送",
        "音楽",
        "http://localhost:8080/",
    );
    assert_eq!(id_1_jp.to_hex_lower(), "ec5efd997acb9315a186f7d4d293a033");

    // Vector 6: 1-length_boundary (IPv6 network)
    let id_1_boundary = derive_broadcast_channel_id(
        &bcid,
        ChannelNetworkType::IPv6,
        &name_128,
        &genre_127,
        &source_jp,
    );
    assert_eq!(
        id_1_boundary.to_hex_lower(),
        "767d964dcbe64ad0a1ab6930f8a5ddb4"
    );
}
