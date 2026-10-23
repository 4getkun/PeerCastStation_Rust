use std::time::Duration;

use peercast_core::channel_id::ChannelId;
use peercast_core::index_txt::{
    html_decode, parse_index_txt, parse_uptime_str, IndexParseError, IndexParserLimits,
};

/// Acceptance criterion 1:
/// Standard 19-column Station/PeerCast-YT index.txt line parsing.
#[test]
fn test_p35_standard_index_txt_parsing() {
    let line = "My Channel<>0123456789ABCDEF0123456789ABCDEF<>192.168.1.10:7144<>http://example.com<>J-Pop<>24/7 Music<>12<>3<>128<>FLV<>Artist Name<>Album Name<>Song Title<>http://track.url<>my_channel<>01:30<>click<>Test comment<>free<>\n";

    let entries = parse_index_txt(line, &IndexParserLimits::default()).unwrap();
    assert_eq!(entries.len(), 1);

    let e = &entries[0];
    assert_eq!(e.name, "My Channel");
    assert_eq!(
        e.channel_id,
        ChannelId::from_hex("0123456789ABCDEF0123456789ABCDEF").unwrap()
    );
    assert_eq!(e.tracker, Some("192.168.1.10:7144".to_string()));
    assert_eq!(e.contact_url, Some("http://example.com".to_string()));
    assert_eq!(e.genre, "J-Pop");
    assert_eq!(e.description, "24/7 Music");
    assert_eq!(e.listeners, Some(12));
    assert_eq!(e.relays, Some(3));
    assert_eq!(e.bitrate_kbps, Some(128));
    assert_eq!(e.content_type, "FLV");
    assert_eq!(e.artist, Some("Artist Name".to_string()));
    assert_eq!(e.album, Some("Album Name".to_string()));
    assert_eq!(e.track_title, Some("Song Title".to_string()));
    assert_eq!(e.track_url, Some("http://track.url".to_string()));
    assert_eq!(e.uptime, Some(Duration::from_secs(5400))); // 1h 30m = 5400s
    assert_eq!(e.comment, Some("Test comment".to_string()));
    assert_eq!(e.raw_token_count, 20); // 19 delimiters + trailing empty
}

/// Acceptance criterion 2:
/// HTML entity decoding for named entities, decimal, and hexadecimal codes.
#[test]
fn test_p35_html_entity_decoding() {
    let raw = "Rock &amp; Roll &lt;Live&gt; &quot;Special&quot; &#39;Edition&#39; &#x2603; &#65;";
    let decoded = html_decode(raw);
    assert_eq!(decoded, "Rock & Roll <Live> \"Special\" 'Edition' ☃ A");

    // Plain text without ampersand should return unchanged
    assert_eq!(html_decode("Simple Text"), "Simple Text");

    // Unterminated or invalid entity should not crash
    assert_eq!(html_decode("&unterminated"), "&unterminated");
    assert_eq!(html_decode("bad &#xyz; entity"), "bad &#xyz; entity");
}

/// Acceptance criterion 3:
/// Uptime parser variants: HH:MM, HH:MM:SS, seconds, and invalid values.
#[test]
fn test_p35_uptime_parsing_variants() {
    // HH:MM
    assert_eq!(parse_uptime_str("01:30"), Some(Duration::from_secs(5400)));
    assert_eq!(parse_uptime_str("00:45"), Some(Duration::from_secs(2700)));

    // HH:MM:SS
    assert_eq!(
        parse_uptime_str("02:15:30"),
        Some(Duration::from_secs(2 * 3600 + 15 * 60 + 30))
    );

    // Raw seconds
    assert_eq!(parse_uptime_str("3600"), Some(Duration::from_secs(3600)));

    // Blank or malformed
    assert_eq!(parse_uptime_str(""), None);
    assert_eq!(parse_uptime_str("   "), None);
    assert_eq!(parse_uptime_str("invalid:time:format:extra"), None);
    assert_eq!(parse_uptime_str("abc"), None);
}

/// Acceptance criterion 4:
/// Partial and optional columns (lines with fewer than 18 columns).
#[test]
fn test_p35_partial_and_optional_columns() {
    let feed = "Minimal Channel<>AABBCCDDEEFF00112233445566778899<>10.0.0.1:7144\n";

    let entries = parse_index_txt(feed, &IndexParserLimits::default()).unwrap();
    assert_eq!(entries.len(), 1);

    let e = &entries[0];
    assert_eq!(e.name, "Minimal Channel");
    assert_eq!(
        e.channel_id,
        ChannelId::from_hex("AABBCCDDEEFF00112233445566778899").unwrap()
    );
    assert_eq!(e.tracker, Some("10.0.0.1:7144".to_string()));
    assert_eq!(e.contact_url, None);
    assert_eq!(e.listeners, None);
    assert_eq!(e.relays, None);
    assert_eq!(e.bitrate_kbps, None);
    assert_eq!(e.content_type, "");
    assert_eq!(e.artist, None);
    assert_eq!(e.uptime, None);
}

/// Acceptance criterion 5:
/// Response total size limit rejection (REQ-063, Global Rule 3).
#[test]
fn test_p35_response_size_limit_rejection() {
    let limits = IndexParserLimits {
        max_total_bytes: 50,
        max_lines: 100,
        max_field_len: 100,
    };

    let large_feed = "This is a response that definitely exceeds the configured maximum limit of fifty bytes.\n";
    let res = parse_index_txt(large_feed, &limits);

    match res {
        Err(IndexParseError::ResponseTooLarge { size, limit }) => {
            assert!(size > 50);
            assert_eq!(limit, 50);
        }
        _ => panic!("Expected ResponseTooLarge error"),
    }
}

/// Acceptance criterion 6:
/// Per-field length limit rejection (REQ-063, Global Rule 3).
#[test]
fn test_p35_field_length_limit_rejection() {
    let limits = IndexParserLimits {
        max_total_bytes: 10000,
        max_lines: 100,
        max_field_len: 10,
    };

    let feed_with_long_field = "Short<>0123456789abcdef0123456789abcdef<>10.0.0.1:7144<>ThisContactUrlIsWayTooLongForTenBytes<>\n";
    let res = parse_index_txt(feed_with_long_field, &limits);

    match res {
        Err(IndexParseError::FieldTooLong { line, field, len, limit }) => {
            assert_eq!(line, 1);
            // Field 1 is 32 bytes hex GUID which exceeds 10 bytes
            assert!(len > 10);
            assert_eq!(limit, 10);
            assert_eq!(field, 1);
        }
        _ => panic!("Expected FieldTooLong error"),
    }
}

/// Acceptance criterion 7:
/// Line count bound (stops at max_lines).
#[test]
fn test_p35_max_lines_bound() {
    let limits = IndexParserLimits {
        max_total_bytes: 10000,
        max_lines: 2, // Only parse up to 2 lines
        max_field_len: 1000,
    };

    let multiline_feed = "\
Ch1<>11111111111111111111111111111111\n\
Ch2<>22222222222222222222222222222222\n\
Ch3<>33333333333333333333333333333333\n\
Ch4<>44444444444444444444444444444444\n";

    let entries = parse_index_txt(multiline_feed, &limits).unwrap();
    assert_eq!(entries.len(), 2, "Must cap at max_lines (2)");
    assert_eq!(entries[0].name, "Ch1");
    assert_eq!(entries[1].name, "Ch2");
}

/// Acceptance criterion 8:
/// Resilience against malformed tokens and full UTF-8 Japanese support.
#[test]
fn test_p35_malformed_tokens_and_japanese_utf8() {
    // Row 1: Japanese characters and HTML entity
    // Row 2: Malformed hex GUID and non-numeric listeners/relays
    let feed = "\
日本語配信チャンネル<>00112233445566778899AABBCCDDEEFF<>192.168.0.1:7144<><>アニメ &amp; ゲーム<>高画質配信中<>100<>50<>1500<>FLV\n\
Malformed GUID Line<>NOT_HEX_GUID<>tip<><><><>NOT_NUM<>NOT_NUM<>NOT_BITR<>MKV\n\
\n\
";

    let entries = parse_index_txt(feed, &IndexParserLimits::default()).unwrap();
    assert_eq!(entries.len(), 2);

    // Row 1: Japanese UTF-8 preserved
    assert_eq!(entries[0].name, "日本語配信チャンネル");
    assert_eq!(entries[0].genre, "アニメ & ゲーム");
    assert_eq!(entries[0].description, "高画質配信中");
    assert_eq!(entries[0].listeners, Some(100));
    assert_eq!(entries[0].relays, Some(50));
    assert_eq!(entries[0].bitrate_kbps, Some(1500));
    assert_eq!(entries[0].content_type, "FLV");

    // Row 2: Non-hex GUID defaults safely without panic; non-numeric numbers parse as None
    assert_eq!(entries[1].name, "Malformed GUID Line");
    assert_eq!(entries[1].channel_id, ChannelId::default());
    assert_eq!(entries[1].listeners, None);
    assert_eq!(entries[1].relays, None);
    assert_eq!(entries[1].bitrate_kbps, None);
    assert_eq!(entries[1].content_type, "MKV");
}
