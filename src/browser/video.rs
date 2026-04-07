// ── Video detection and external launch ───────────────────────────────────────

#[derive(Debug, Clone)]
pub enum VideoType {
    Direct,
    YouTube,
    Hls,
    Dash,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct VideoInfo {
    pub title: String,
    pub src: String,
    pub video_type: VideoType,
}

pub fn detect_videos(html: &str, page_url: &str) -> Vec<VideoInfo> {
    let mut videos: Vec<VideoInfo> = Vec::new();

    if page_url.contains("youtube.com/watch") || page_url.contains("youtu.be/") {
        if let Some(id) = extract_youtube_id(page_url) {
            videos.push(VideoInfo {
                title: "YouTube Video".to_string(),
                src: format!("https://www.youtube.com/watch?v={}", id),
                video_type: VideoType::YouTube,
            });
        }
    }

    if html.contains(".m3u8") {
        if let Some(url) = extract_manifest_url(html, ".m3u8") {
            videos.push(VideoInfo {
                title: "Video Stream (HLS)".to_string(),
                src: url,
                video_type: VideoType::Hls,
            });
        }
    }

    if html.contains(".mpd") {
        if let Some(url) = extract_manifest_url(html, ".mpd") {
            videos.push(VideoInfo {
                title: "Video Stream (DASH)".to_string(),
                src: url,
                video_type: VideoType::Dash,
            });
        }
    }

    let mut search = html;
    while let Some(pos) = search.find("<video") {
        search = &search[pos + 6..];
        if let Some(src) = attr_value(search, "src") {
            if !src.is_empty() && !src.contains("googlevideo.com") {
                videos.push(VideoInfo {
                    title: "Video".to_string(),
                    src: src.clone(),
                    video_type: classify(&src),
                });
            }
        }
        if let Some(end) = search.find('>') {
            search = &search[end + 1..];
        } else {
            break;
        }
    }

    let mut seen = std::collections::HashSet::new();
    videos.retain(|v| seen.insert(v.src.clone()));
    videos
}

fn classify(url: &str) -> VideoType {
    if url.contains(".m3u8") {
        VideoType::Hls
    } else if url.contains(".mpd") {
        VideoType::Dash
    } else if url.contains("youtube.com") || url.contains("youtu.be") {
        VideoType::YouTube
    } else {
        VideoType::Direct
    }
}

fn extract_youtube_id(url: &str) -> Option<String> {
    if let Ok(parsed) = url::Url::parse(url) {
        if let Some((_, v)) = parsed.query_pairs().find(|(k, _)| k == "v") {
            return Some(v.to_string());
        }
        let path = parsed.path().trim_start_matches('/');
        if !path.is_empty() && !path.contains('/') {
            return Some(path.to_string());
        }
    }
    None
}

fn extract_manifest_url(html: &str, ext: &str) -> Option<String> {
    let idx = html.find(ext)?;
    let before = &html[..idx];
    let start = before.rfind(|c| c == '"' || c == '\'')? + 1;
    let end = idx + ext.len();
    let url = &html[start..end];
    if url.starts_with("http") || url.starts_with('/') {
        Some(url.to_string())
    } else {
        None
    }
}

fn attr_value(frag: &str, attr: &str) -> Option<String> {
    let needle = format!("{}=\"", attr);
    let start = frag.find(&needle)? + needle.len();
    let end = frag[start..].find('"')? + start;
    Some(frag[start..end].to_string())
}

// ── Launch ────────────────────────────────────────────────────────────────────

pub fn launch_video(video: &VideoInfo) -> Result<(), String> {
    match video.video_type {
        VideoType::YouTube => launch_youtube(&video.src),
        _ => launch_local(&video.src),
    }
}

/// For YouTube: use yt-dlp to download to a temp .mp4 file, then open that file.
/// Streaming URLs from yt-dlp --get-url don't work in WMP (auth tokens, codec issues).
/// Downloading first works with every player including WMP.
fn launch_youtube(watch_url: &str) -> Result<(), String> {
    let tmp = temp_video_path();

    // Download with yt-dlp to a temp file
    let ytdlp = find_ytdlp()
        .ok_or_else(|| "yt-dlp not found. Install with: pip install yt-dlp".to_string())?;

    let status = std::process::Command::new(&ytdlp)
        .args([
            "-f",
            "best[height<=720][ext=mp4]/best[ext=mp4]/best",
            "--no-playlist",
            "-o",
            &tmp,
            "--no-part",
            watch_url,
        ])
        .status()
        .map_err(|e| format!("yt-dlp failed to start: {}", e))?;

    if !status.success() {
        return Err(
            "yt-dlp download failed. Check the URL or your internet connection.".to_string(),
        );
    }

    launch_local(&tmp)
}

fn find_ytdlp() -> Option<String> {
    // Common locations on Windows
    let candidates = [
        "yt-dlp",
        "yt-dlp.exe",
        r"C:\Users\namit\AppData\Roaming\Python\Python313\Scripts\yt-dlp.exe",
        r"C:\Python313\Scripts\yt-dlp.exe",
        r"C:\Python312\Scripts\yt-dlp.exe",
        r"C:\Python311\Scripts\yt-dlp.exe",
        r"C:\Python310\Scripts\yt-dlp.exe",
    ];
    for c in &candidates {
        if std::process::Command::new(c)
            .arg("--version")
            .output()
            .is_ok()
        {
            return Some(c.to_string());
        }
    }
    None
}

fn temp_video_path() -> String {
    let tmp = std::env::var("TEMP")
        .or_else(|_| std::env::var("TMP"))
        .unwrap_or_else(|_| ".".to_string());
    format!("{}\\az_video.mp4", tmp)
}

/// Open a direct URL in a local media player (never a browser).
fn launch_local(url: &str) -> Result<(), String> {
    // Try players in order of preference
    let players: &[(&str, &[&str])] = &[
        ("mpv", &[]),
        ("vlc", &[]),
        ("mplayer", &[]),
        // Windows Media Player — full path needed since it's not usually in PATH
        ("wmplayer", &[]),
        (r"C:\Program Files\Windows Media Player\wmplayer.exe", &[]),
        (
            r"C:\Program Files (x86)\Windows Media Player\wmplayer.exe",
            &[],
        ),
        // Pot Player, MPC-HC — common Windows players
        ("PotPlayerMini64", &[]),
        ("PotPlayerMini", &[]),
        (r"C:\Program Files\DAUM\PotPlayer\PotPlayerMini64.exe", &[]),
        (
            r"C:\Program Files (x86)\K-Lite Codec Pack\MPC-HC64\mpc-hc64.exe",
            &[],
        ),
        (r"C:\Program Files\MPC-HC\mpc-hc64.exe", &[]),
    ];

    for (player, extra_args) in players {
        let mut cmd = std::process::Command::new(player);
        cmd.args(*extra_args);
        cmd.arg(url);
        if cmd.spawn().is_ok() {
            return Ok(());
        }
    }

    Err(
        "No media player found. Install mpv (recommended), VLC, or Windows Media Player.\n\
         mpv: https://mpv.io  |  yt-dlp: pip install yt-dlp"
            .to_string(),
    )
}
