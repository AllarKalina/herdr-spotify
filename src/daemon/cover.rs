//! Cover images, fetched once and kept as files for the popup to draw.

use crate::Result;
use crate::paths::Paths;
use std::fs;
use std::path::Path;
use std::time::Duration;

/// How many covers stay on disk; the oldest go first.
const KEEP: usize = 40;
const TIMEOUT: Duration = Duration::from_secs(10);
/// Far above any real cover, and small enough that a wrong URL cannot fill the disk.
const SIZE_LIMIT: usize = 2 * 1024 * 1024;

/// The local file for the cover at `url`, downloading it on first use.
pub async fn fetch(paths: &Paths, url: &str) -> Result<String> {
    let name = file_name(url).ok_or("unexpected cover URL")?;
    let dir = paths.covers();
    let path = dir.join(format!("{name}.jpg"));
    if !path.exists() {
        let client = reqwest::Client::builder().timeout(TIMEOUT).build()?;
        let image = client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        if image.len() > SIZE_LIMIT {
            return Err("cover image is implausibly large".into());
        }
        fs::create_dir_all(&dir)?;
        let partial = dir.join(format!("{name}.partial"));
        fs::write(&partial, &image)?;
        fs::rename(&partial, &path)?;
        prune(&dir);
    }
    Ok(path.to_string_lossy().into_owned())
}

/// Spotify names cover images by a hex ID, the last segment of the URL. Anything else is
/// refused rather than turned into a file name.
fn file_name(url: &str) -> Option<&str> {
    let name = url.strip_prefix("https://")?.rsplit('/').next()?;
    let plain =
        !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric());
    plain.then_some(name)
}

fn prune(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut covers: Vec<_> = entries
        .flatten()
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    covers.sort();
    let excess = covers.len().saturating_sub(KEEP);
    for (_, path) in covers.into_iter().take(excess) {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_image_id_becomes_a_file_name() {
        let id = "ab67616d00004851d5a0eaff23dc8cde8f9dec48";
        assert_eq!(
            file_name(&format!("https://i.scdn.co/image/{id}")),
            Some(id)
        );
        assert_eq!(
            file_name("https://i.scdn.co/image/../../etc/passwd"),
            Some("passwd")
        );
        assert_eq!(file_name("https://i.scdn.co/image/a.b"), None);
        assert_eq!(file_name("https://i.scdn.co/image/"), None);
        assert_eq!(file_name("http://i.scdn.co/image/abc"), None);
    }

    #[test]
    fn pruning_keeps_the_newest_covers() {
        let dir = std::path::PathBuf::from(format!("/tmp/hs-covers-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        for index in 0..KEEP + 3 {
            let path = dir.join(format!("{index:03}.jpg"));
            fs::write(&path, b"x").expect("write");
            let time = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(index as u64 + 1);
            fs::File::options()
                .write(true)
                .open(&path)
                .and_then(|f| f.set_modified(time))
                .expect("time");
        }
        prune(&dir);
        assert_eq!(fs::read_dir(&dir).expect("read").count(), KEEP);
        assert!(!dir.join("000.jpg").exists());
        assert!(dir.join(format!("{:03}.jpg", KEEP + 2)).exists());
        fs::remove_dir_all(&dir).expect("clean");
    }
}
