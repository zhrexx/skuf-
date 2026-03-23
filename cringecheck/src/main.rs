// cringecheck — a cringe-yet-interesting CLI tool
// Analyses any text and awards it a "cringe score" with a personality verdict.
//
// Usage:
//   cringecheck [TEXT...]
//   echo "some text" | cringecheck
//
// If no arguments and no stdin pipe are detected the tool runs in
// interactive mode and asks the user to type something.

use std::io::{self, BufRead, IsTerminal, Write};
use std::thread;
use std::time::Duration;

// ── colour helpers (ANSI, no external deps) ──────────────────────────────────

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
const MAGENTA: &str = "\x1b[35m";
const BRIGHT_WHITE: &str = "\x1b[97m";

fn colour(c: &str, s: &str) -> String {
    format!("{c}{s}{RESET}")
}

// ── ASCII art banners ─────────────────────────────────────────────────────────

const BANNER: &str = r#"
  ██████╗██████╗ ██╗███╗   ██╗ ██████╗ ███████╗
 ██╔════╝██╔══██╗██║████╗  ██║██╔════╝ ██╔════╝
 ██║     ██████╔╝██║██╔██╗ ██║██║  ███╗█████╗  
 ██║     ██╔══██╗██║██║╚██╗██║██║   ██║██╔══╝  
 ╚██████╗██║  ██║██║██║ ╚████║╚██████╔╝███████╗
  ╚═════╝╚═╝  ╚═╝╚═╝╚═╝  ╚═══╝ ╚═════╝ ╚══════╝
 ██████╗██╗  ██╗███████╗ ██████╗██╗  ██╗
██╔════╝██║  ██║██╔════╝██╔════╝██║ ██╔╝
██║     ███████║█████╗  ██║     █████╔╝ 
██║     ██╔══██║██╔══╝  ██║     ██╔═██╗ 
╚██████╗██║  ██║███████╗╚██████╗██║  ██╗
 ╚═════╝╚═╝  ╚═╝╚══════╝ ╚═════╝╚═╝  ╚═╝
           ~ the vibe analyser ~
"#;

// ── scoring & titles ──────────────────────────────────────────────────────────

#[derive(Debug)]
struct Analysis {
    score: f64,    // 0.0 – 100.0
    caps_ratio: f64,
    exclaim_count: usize,
    emoji_count: usize,
    buzzword_hits: Vec<&'static str>,
    length: usize,
    repeated_chars: usize,
}

fn count_emojis(s: &str) -> usize {
    // Very rough emoji detector: any non-ASCII codepoint above U+2600
    s.chars()
        .filter(|&c| c as u32 >= 0x2600)
        .count()
}

fn count_repeated_chars(s: &str) -> usize {
    // Counts runs of 3+ identical chars (e.g. "sooooo", "!!!!")
    let chars: Vec<char> = s.chars().collect();
    let mut count = 0;
    let mut run = 1usize;
    for i in 1..chars.len() {
        if chars[i] == chars[i - 1] {
            run += 1;
            if run == 3 {
                count += 1; // first qualifying run
            }
        } else {
            run = 1;
        }
    }
    count
}

const BUZZWORDS: &[&str] = &[
    // hustle / sigma culture
    "sigma",
    "alpha",
    "grindset",
    "based",
    "lowkey",
    "highkey",
    "no cap",
    "fr fr",
    "bussin",
    "slay",
    "rizz",
    "vibe",
    "mid",
    "goated",
    "ngl",
    "npc",
    "main character",
    "understood the assignment",
    "hits different",
    "it's giving",
    "the way",
    "periodt",
    "bestie",
    "queen",
    "king",
    "yass",
    "omg",
    "lmao",
    "lmfao",
    "bruh",
    "ratio",
    "touch grass",
    "cope",
    "seethe",
    "mald",
    "gg",
    "poggers",
    "uwu",
    "owo",
    "thx",
    "pog",
    "sheesh",
    // generic net-speak
    "xd",
    "lol",
    "rofl",
    "tbh",
    "smh",
    "imo",
    "fwiw",
    "idk",
    "ikr",
    "bro",
    "bff",
    "fam",
    "lit",
    "fire",
    "w",
    "l",
    "valid",
    "stan",
    "ate",
    "served",
    "iconic",
    "core",
    "era",
    // extra cringe special sauce
    "literally",
    "obsessed",
    "dead",
    "crying",
    "screaming",
    "not me",
    "okay but",
    "i can't",
    "i'm weak",
    "ratioed",
    "touch some grass",
    "respectfully",
    "delulu",
    "snatched",
    "it's a vibe",
    "rent free",
];

fn find_buzzwords(lower: &str) -> Vec<&'static str> {
    BUZZWORDS
        .iter()
        .filter(|&&w| {
            // Multi-word phrases can rely on a plain substring search;
            // single/few-char tokens need word-boundary guards so "l"
            // doesn't match inside "quarterly" etc.
            if w.contains(' ') {
                lower.contains(w)
            } else {
                // Check that the match is surrounded by non-alphanumeric chars
                // (or string boundaries).
                let mut start = 0;
                while let Some(pos) = lower[start..].find(w) {
                    let abs = start + pos;
                    let before_ok = abs == 0
                        || !lower[..abs]
                            .chars()
                            .last()
                            .map(|c| c.is_alphanumeric())
                            .unwrap_or(false);
                    let after = abs + w.len();
                    let after_ok = after >= lower.len()
                        || !lower[after..]
                            .chars()
                            .next()
                            .map(|c| c.is_alphanumeric())
                            .unwrap_or(false);
                    if before_ok && after_ok {
                        return true;
                    }
                    start = abs + 1;
                }
                false
            }
        })
        .copied()
        .collect()
}

fn analyse(text: &str) -> Analysis {
    let total_chars = text.chars().count();
    let alpha_chars: Vec<char> = text.chars().filter(|c| c.is_alphabetic()).collect();
    let upper_count = alpha_chars.iter().filter(|c| c.is_uppercase()).count();
    let caps_ratio = if alpha_chars.is_empty() {
        0.0
    } else {
        upper_count as f64 / alpha_chars.len() as f64
    };

    let exclaim_count = text.chars().filter(|&c| c == '!').count();
    let emoji_count = count_emojis(text);
    let lower = text.to_lowercase();
    let buzzword_hits = find_buzzwords(&lower);
    let repeated_chars = count_repeated_chars(text);

    // ── scoring ───────────────────────────────────────────────────────────────
    // Each factor contributes up to a capped amount so no single axis
    // can max out the score on its own.
    let caps_points = (caps_ratio * 30.0).min(25.0);
    let exclaim_points = (exclaim_count as f64 * 3.0).min(20.0);
    let emoji_points = (emoji_count as f64 * 4.0).min(20.0);
    let buzz_points = (buzzword_hits.len() as f64 * 4.0).min(25.0);
    let repeat_points = (repeated_chars as f64 * 5.0).min(15.0);
    // Very short texts get a small bonus — even a 1-word "lol" is already
    // peak cringe and should not score 0 just because it's tiny.
    let length_bonus = if total_chars < 5 { 5.0 } else { 0.0 };

    let raw = caps_points + exclaim_points + emoji_points + buzz_points + repeat_points + length_bonus;
    let score = raw.min(100.0);

    Analysis {
        score,
        caps_ratio,
        exclaim_count,
        emoji_count,
        buzzword_hits,
        length: total_chars,
        repeated_chars,
    }
}

// ── verdict mapping ───────────────────────────────────────────────────────────

struct Verdict {
    title: &'static str,
    emoji: &'static str,
    flavour: &'static str,
    colour_fn: fn(&str) -> String,
}

fn get_verdict(score: f64) -> Verdict {
    match score as u32 {
        0..=9 => Verdict {
            title: "EMOTIONLESS ROBOT",
            emoji: "🤖",
            flavour: "Bro typed like a legal document. Respect, but also… seek help.",
            colour_fn: |s| colour(BRIGHT_WHITE, s),
        },
        10..=24 => Verdict {
            title: "SILENT NPC",
            emoji: "😐",
            flavour: "You radiate 'I have nothing to say but I said it anyway' energy.",
            colour_fn: |s| colour(CYAN, s),
        },
        25..=39 => Verdict {
            title: "CHRONICALLY ONLINE SIDE CHARACTER",
            emoji: "💀",
            flavour: "You're not the main character yet, but you're clearly watching too much TikTok.",
            colour_fn: |s| colour(GREEN, s),
        },
        40..=54 => Verdict {
            title: "CERTIFIED BRAINROT ENJOYER",
            emoji: "🧠",
            flavour: "The internet has colonised your thoughts. No cure known to science.",
            colour_fn: |s| colour(YELLOW, s),
        },
        55..=69 => Verdict {
            title: "MAIN CHARACTER ENERGY",
            emoji: "✨",
            flavour: "You understood the assignment. Lowkey iconic, no cap, periodt.",
            colour_fn: |s| colour(MAGENTA, s),
        },
        70..=84 => Verdict {
            title: "SIGMA GRINDSET ACTIVATED",
            emoji: "😤",
            flavour: "You're in your delulu era and we respect the hustle, bestie. Slay.",
            colour_fn: |s| colour(RED, s),
        },
        _ => Verdict {
            title: "ABSOLUTE UNHINGED LEGEND",
            emoji: "🔥",
            flavour: "Science cannot explain what just happened. You are built different. We're scared.",
            colour_fn: |s| format!("{BOLD}{}{RESET}", colour(RED, s)),
        },
    }
}

// ── fake loading bar ──────────────────────────────────────────────────────────

fn loading_bar() {
    let messages = [
        "scanning vibes",
        "measuring the cringe",
        "consulting the sigma oracle",
        "running through buzzword database",
        "calculating your rizz coefficient",
        "it's giving... loading",
        "no cap, this takes a sec",
    ];

    let msg = messages[rand_usize() % messages.len()];
    print!("{}", colour(CYAN, &format!("  {msg}: [")));
    io::stdout().flush().unwrap();

    for _ in 0..20 {
        print!("{}", colour(MAGENTA, "█"));
        io::stdout().flush().unwrap();
        thread::sleep(Duration::from_millis(30));
    }
    println!("{}", colour(CYAN, "] done!"));
}

// Extremely minimal LCG pseudo-random (no external deps)
fn rand_usize() -> usize {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(42);
    // LCG: next = (a * seed + c)
    (1664525u64.wrapping_mul(seed as u64).wrapping_add(1013904223)) as usize
}

// ── report printer ────────────────────────────────────────────────────────────

fn print_report(text: &str, a: &Analysis) {
    let verdict = get_verdict(a.score);

    // Score bar
    let filled = (a.score / 5.0) as usize; // max 20 blocks
    let bar: String = "█".repeat(filled) + &"░".repeat(20 - filled);

    println!();
    println!("{}", colour(BOLD, "┌─────────────────────────────────────────┐"));
    println!("{}", colour(BOLD, "│            CRINGE REPORT                │"));
    println!("{}", colour(BOLD, "└─────────────────────────────────────────┘"));
    println!();

    // Analysed text (truncated if long)
    let display_text = if text.chars().count() > 60 {
        let t: String = text.chars().take(57).collect();
        format!("{t}...")
    } else {
        text.to_owned()
    };
    println!("  {} {}", colour(CYAN, "Input:"), colour(BRIGHT_WHITE, &format!("\"{}\"", display_text)));
    println!();

    // Score
    println!(
        "  {} {}{}/100{}",
        colour(CYAN, "Score:"),
        colour(MAGENTA, &format!("[{bar}] ")),
        colour(BOLD, &format!("{:.1}", a.score)),
        RESET,
    );
    println!();

    // Verdict
    println!(
        "  {} {} {}",
        colour(CYAN, "Verdict:"),
        verdict.emoji,
        (verdict.colour_fn)(&format!("{}", verdict.title)),
    );
    println!();
    println!("  {}", colour(YELLOW, verdict.flavour));
    println!();

    // Breakdown
    println!("{}", colour(BOLD, "  ── Breakdown ──────────────────────────────"));
    let breakdown: Vec<(String, String)> = vec![
        ("Chars".to_owned(), a.length.to_string()),
        ("CAPS ratio".to_owned(), format!("{:.0}%", a.caps_ratio * 100.0)),
        ("Exclamation marks".to_owned(), a.exclaim_count.to_string()),
        ("Emojis detected".to_owned(), a.emoji_count.to_string()),
        ("Repeated-char runs".to_owned(), a.repeated_chars.to_string()),
        ("Buzzwords found".to_owned(), a.buzzword_hits.len().to_string()),
    ];
    for (k, v) in &breakdown {
        println!("  {:<22} {}", colour(CYAN, k), colour(BRIGHT_WHITE, v));
    }

    if !a.buzzword_hits.is_empty() {
        let hits: String = a.buzzword_hits.join(", ");
        println!("  {:<22} {}", colour(CYAN, "Buzzword list"), colour(MAGENTA, &hits));
    }
    println!();

    // Fun facts
    if a.caps_ratio > 0.5 {
        println!("  {} Please release the Caps Lock key. It doesn't help.", colour(RED, "⚠️"));
    }
    if a.exclaim_count >= 5 {
        println!("  {} {} exclamation marks. Calm down bestie.", colour(YELLOW, "❗"), a.exclaim_count);
    }
    if a.emoji_count >= 5 {
        println!("  {} {} emojis. Your keyboard is working, we promise.", colour(YELLOW, "🎉"), a.emoji_count);
    }
    if a.repeated_chars >= 3 {
        println!("  {} Repeated characters detected. We felt that.", colour(YELLOW, "🔁"));
    }

    println!();
    println!("{}", colour(BOLD, "─────────────────────────────────────────────"));
}

// ── entry point ───────────────────────────────────────────────────────────────

fn main() {
    // Print banner
    println!("{}", colour(MAGENTA, BANNER));

    // Collect input
    let args: Vec<String> = std::env::args().skip(1).collect();
    let text: String;

    if !args.is_empty() {
        // Text supplied as CLI arguments
        text = args.join(" ");
    } else if !io::stdin().is_terminal() {
        // Reading from a pipe / redirect
        let stdin = io::stdin();
        let mut lines = Vec::new();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => lines.push(l),
                Err(_) => break,
            }
        }
        text = lines.join("\n");
    } else {
        // Interactive mode
        println!("{}", colour(CYAN, "  No input detected — entering interactive mode!"));
        println!("{}", colour(BRIGHT_WHITE, "  Type anything and press ENTER:"));
        print!("  {} ", colour(MAGENTA, ">"));
        io::stdout().flush().unwrap();

        let stdin = io::stdin();
        let mut buf = String::new();
        stdin.lock().read_line(&mut buf).unwrap();
        text = buf.trim().to_owned();
    }

    if text.trim().is_empty() {
        println!("{}", colour(RED, "  Nothing to analyse. Come on bestie, type something. 😭"));
        std::process::exit(1);
    }

    println!();
    loading_bar();
    println!();

    let analysis = analyse(&text);
    print_report(&text, &analysis);
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_low_score_plain_text() {
        let a = analyse("The quarterly report shows marginal growth.");
        assert!(a.score < 20.0, "plain text should score low, got {}", a.score);
    }

    #[test]
    fn test_high_score_cringe_text() {
        let a = analyse("OMGGGG BESTIE THIS IS SOOOOO BUSSIN FR FR NO CAP!!!!!!! 🔥🔥🔥✨✨");
        assert!(a.score > 50.0, "cringe text should score high, got {}", a.score);
    }

    #[test]
    fn test_caps_ratio() {
        let a = analyse("HELLO WORLD");
        assert!(a.caps_ratio > 0.9);
    }

    #[test]
    fn test_exclaim_count() {
        let a = analyse("Wow!!!!! so cool!!");
        assert_eq!(a.exclaim_count, 7);
    }

    #[test]
    fn test_emoji_count() {
        let a = analyse("cool 🔥✨💀🤖😐");
        assert_eq!(a.emoji_count, 5);
    }

    #[test]
    fn test_buzzword_hits() {
        let a = analyse("no cap fr fr bussin slay periodt bestie");
        assert!(a.buzzword_hits.len() >= 5);
    }

    #[test]
    fn test_repeated_chars() {
        let a = analyse("sooooo coooool yaaas");
        assert!(a.repeated_chars >= 2);
    }

    #[test]
    fn test_score_capped_at_100() {
        let a = analyse(
            "OMGGGG!!!!!!! SOOOOO BASED SIGMA GRINDSET FR FR NO CAP BUSSIN SLAY \
             RIZZ VIBE BESTIE QUEEN KING YASS OMG BRUH GOATED 🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥✨✨✨✨",
        );
        assert!(a.score <= 100.0);
    }

    #[test]
    fn test_empty_string_scores_low() {
        let a = analyse("");
        // caps ratio should be 0, length bonus only
        assert!(a.score <= 10.0);
    }

    #[test]
    fn test_verdict_bands() {
        // Each band should produce a distinct title
        let titles: Vec<&str> = [0.0f64, 15.0, 30.0, 47.0, 60.0, 77.0, 95.0]
            .iter()
            .map(|&s| get_verdict(s).title)
            .collect();
        // All titles should be unique (7 distinct bands)
        let unique: std::collections::HashSet<&&str> = titles.iter().collect();
        assert_eq!(unique.len(), 7);
    }
}
