//! Read-only, explicit network check for the bundled live-monitor extractor.
#[tokio::main]
async fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let tool = args.next().ok_or("yt-dlp executable is required")?;
    let page = args.next().ok_or("channel URL is required")?;
    let data = if page.contains("twitch.tv/") {
        peercast_desktop::twitch_process::probe(std::path::Path::new(&tool), &page).await?
    } else {
        peercast_desktop::youtube_process::probe(std::path::Path::new(&tool), &page).await?
    };
    println!(
        "{}",
        serde_json::json!({"name":data.name,"channelId":data.channel_id,"streams":data.streams.iter().map(|stream| serde_json::json!({"id":stream.id,"status":stream.status})).collect::<Vec<_>>()})
    );
    Ok(())
}
