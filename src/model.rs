use std::collections::{HashSet, VecDeque};
use unicode_normalization::UnicodeNormalization;

pub const MAX_QUEUE: usize = 200;
pub const OUTPUT_GAIN: f32 = 0.5;

#[derive(Clone, Debug)]
pub struct Media {
    pub title: String,
    pub artist: String,
    pub url: String,
    pub duration: Option<f64>,
    pub verified: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    End,
    Next,
    Now,
}

pub fn enqueue(
    queue: &mut VecDeque<Media>,
    batch: Vec<Media>,
    position: Position,
) -> Result<(), &'static str> {
    if queue.len() + batch.len() > MAX_QUEUE {
        return Err("Queue limit is 200 tracks.");
    }
    match position {
        Position::End => queue.extend(batch),
        Position::Next | Position::Now => {
            for media in batch.into_iter().rev() {
                queue.push_front(media);
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default)]
pub enum Curve {
    #[default]
    Linear,
    EqualPower,
}

pub fn gains(curve: Curve, progress: f64) -> (f32, f32) {
    let t = progress.clamp(0.0, 1.0) as f32;
    let (a, b) = match curve {
        Curve::Linear => (1.0 - t, t),
        Curve::EqualPower => {
            let angle = t * std::f32::consts::FRAC_PI_2;
            (angle.cos().max(0.0), angle.sin())
        }
    };
    (a * OUTPUT_GAIN, b * OUTPUT_GAIN)
}

fn words(s: &str) -> HashSet<String> {
    s.nfkc()
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|x| !x.is_empty())
        .map(str::to_owned)
        .collect()
}

// Metadata hints raise confidence; uploader verification does not prove ownership.
pub fn score(query: &str, media: &Media) -> f64 {
    let query_words = words(query);
    let title = words(&media.title);
    let mut haystack = title.clone();
    haystack.extend(words(&media.artist));
    let overlap =
        query_words.intersection(&haystack).count() as f64 / query_words.len().max(1) as f64;
    let variants = [
        "cover",
        "live",
        "parody",
        "karaoke",
        "ambient",
        "remix",
        "slowed",
        "sped",
        "nightcore",
        "instrumental",
        "reaction",
    ];
    let unwanted = variants
        .iter()
        .any(|v| title.contains(*v) && !query_words.contains(*v));
    let official = title.contains("official")
        || title.contains("audio")
        || words(&media.artist).contains("topic");
    overlap * 0.85 + if official { 0.05 } else { 0.0 } + if media.verified { 0.05 } else { 0.0 }
        - if unwanted { 0.65 } else { 0.0 }
}

pub fn confident(scores: &[f64]) -> bool {
    scores
        .first()
        .is_some_and(|s| *s >= 0.80 && scores.get(1).is_none_or(|next| s - next >= 0.12))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn media(title: &str) -> Media {
        Media {
            title: title.into(),
            artist: "Artist".into(),
            url: "https://www.youtube.com/watch?v=x".into(),
            duration: Some(180.0),
            verified: false,
        }
    }
    #[test]
    fn next_preserves_playlist_order() {
        let mut q = VecDeque::from([media("old")]);
        enqueue(&mut q, vec![media("a"), media("b")], Position::Next).unwrap();
        assert_eq!(
            q.iter().map(|x| x.title.as_str()).collect::<Vec<_>>(),
            ["a", "b", "old"]
        );
    }
    #[test]
    fn queue_limit_is_atomic() {
        let mut q = VecDeque::from(vec![media("x"); 199]);
        assert!(enqueue(&mut q, vec![media("a"), media("b")], Position::End).is_err());
        assert_eq!(q.len(), 199);
    }
    #[test]
    fn unintended_versions_lose() {
        assert!(
            score("Artist Song", &media("Song Official Audio"))
                > score("Artist Song", &media("Song live cover")) + 0.5
        );
        assert!(score("Artist Song live", &media("Song live")) > 0.8);
    }
    #[test]
    fn ambiguous_and_weak_matches_need_selection() {
        assert!(!confident(&[0.95, 0.90]));
        assert!(!confident(&[0.5]));
        assert!(confident(&[0.95, 0.70]));
    }
    #[test]
    fn unicode_query_is_normalized() {
        assert!(score("Ａｒｔｉｓｔ Song", &media("Song")) > 0.8);
    }
    #[test]
    fn crossfade_contains_both_signals_and_keeps_headroom() {
        for curve in [Curve::Linear, Curve::EqualPower] {
            let (a, b) = gains(curve, 0.5);
            assert!(a > 0.0 && b > 0.0);
            assert!(a + b <= 1.0);
            let (start_a, start_b) = gains(curve, 0.0);
            let (end_a, end_b) = gains(curve, 1.0);
            assert_eq!((start_a, start_b), (OUTPUT_GAIN, 0.0));
            assert!(end_a < 0.000001);
            assert_eq!(end_b, OUTPUT_GAIN);
            // Two distinct input frequencies must remain measurable during overlap.
            let n = 4800;
            let mut x_energy = 0.0;
            let mut y_energy = 0.0;
            for i in 0..n {
                let t = i as f64 / 48000.0;
                let x = (t * 440.0 * std::f64::consts::TAU).sin();
                let y = (t * 880.0 * std::f64::consts::TAU).sin();
                let mixed = a as f64 * x + b as f64 * y;
                x_energy += mixed * x;
                y_energy += mixed * y;
            }
            assert!((x_energy / (n as f64 / 2.0) - a as f64).abs() < 0.001);
            assert!((y_energy / (n as f64 / 2.0) - b as f64).abs() < 0.001);
        }
    }
}
