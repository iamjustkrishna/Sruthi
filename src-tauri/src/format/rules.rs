//! Instant, deterministic clean-up of a raw transcript: filler words, spoken
//! punctuation, numbers, codes, times, money, emails and phone numbers.
//!
//! Parakeet already writes most numbers as digits, so these rules mostly
//! fill the gaps it leaves (see the tests for real outputs).

use std::sync::LazyLock;

use regex::{Captures, Regex};

pub fn apply(text: &str) -> String {
    let mut s = collapse_loops(text.trim());
    s = remove_fillers(&s);
    s = voice_commands(&s);
    s = numbered_lists(&s);
    s = number_words(&s);
    s = spelled_codes(&s);
    s = times(&s);
    s = money_and_percent(&s);
    s = emails_and_domains(&s);
    s = phone_numbers(&s);
    tidy(&s)
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid regex")
}

/// Splits a token into (leading punctuation, word, trailing punctuation).
fn split_punct(token: &str) -> (&str, &str, &str) {
    let start = token.find(|c: char| c.is_alphanumeric()).unwrap_or(token.len());
    let end = token
        .rfind(|c: char| c.is_alphanumeric())
        .map(|i| i + token[i..].chars().next().unwrap().len_utf8())
        .unwrap_or(start);
    (&token[..start], &token[start..end], &token[end..])
}

// ---------------------------------------------------------------------------
// Hallucination loops

/// Longest phrase (in words) checked for runaway repetition.
const MAX_LOOP_WORDS: usize = 8;

/// Speech models sometimes get stuck repeating a phrase on quiet or odd
/// audio ("a little bit of a little bit of a little bit of …"). A phrase
/// repeated 3+ times in a row (a single word 4+ times) is kept once.
/// Natural doubles like "very very" are left alone.
fn collapse_loops(s: &str) -> String {
    let words: Vec<&str> = s.split(' ').filter(|w| !w.is_empty()).collect();
    let key = |w: &str| split_punct(w).1.to_lowercase();
    let keys: Vec<String> = words.iter().map(|w| key(w)).collect();

    let mut out: Vec<&str> = Vec::with_capacity(words.len());
    let mut i = 0;
    'outer: while i < words.len() {
        for n in (1..=MAX_LOOP_WORDS).rev() {
            let min_repeats = if n == 1 { 4 } else { 3 };
            if i + n * min_repeats > words.len() || keys[i..i + n].iter().all(String::is_empty) {
                continue;
            }
            let mut repeats = 1;
            while i + (repeats + 1) * n <= words.len()
                && keys[i + repeats * n..i + (repeats + 1) * n] == keys[i..i + n]
            {
                repeats += 1;
            }
            if repeats >= min_repeats {
                out.extend_from_slice(&words[i..i + n]);
                i += repeats * n;
                continue 'outer;
            }
        }
        out.push(words[i]);
        i += 1;
    }
    out.join(" ")
}

// ---------------------------------------------------------------------------
// Fillers

const FILLERS: &[&str] = &["um", "umm", "uh", "uhh", "uhm", "erm", "er", "ah", "ahh", "hmm", "hm", "mm", "mmm"];

fn remove_fillers(s: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for token in s.split(' ') {
        let (_, word, trail) = split_punct(token);
        if FILLERS.contains(&word.to_lowercase().as_str()) {
            // Keep a sentence-ending mark the filler was carrying.
            if trail.contains(['.', '?', '!']) {
                if let Some(prev) = out.last_mut() {
                    if !prev.ends_with(['.', '?', '!', ',']) {
                        prev.push_str(&trail.replace(',', ""));
                    }
                }
            }
            continue;
        }
        out.push(token.to_string());
    }
    out.join(" ")
}

// ---------------------------------------------------------------------------
// Spoken punctuation and layout

static VOICE_COMMANDS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (re(r"(?i)[\s,.]*\bnew paragraph\b[\s,.]*"), "\n\n"),
        (re(r"(?i)[\s,.]*\b(?:new|next) line\b[\s,.]*"), "\n"),
        (re(r"(?i)[\s,.]*\bfull stop\b[.,]?"), "."),
        (re(r"(?i)[\s,.]*\bquestion mark\b[.,]?"), "?"),
        (re(r"(?i)[\s,.]*\bexclamation (?:mark|point)\b[.,]?"), "!"),
        // "comma" as a lone command word, not "a comma" / "the comma".
        (re(r"(?i)(\b(?:a|the)\s+)?[\s,]*\bcomma\b[,.]?"), "\u{0}COMMA"),
    ]
});

fn voice_commands(s: &str) -> String {
    let mut out = s.to_string();
    for (pattern, replacement) in VOICE_COMMANDS.iter() {
        out = if *replacement == "\u{0}COMMA" {
            pattern
                .replace_all(&out, |c: &Captures| match c.get(1) {
                    Some(article) => format!("{}comma", article.as_str()),
                    None => ",".to_string(),
                })
                .into_owned()
        } else {
            pattern.replace_all(&out, *replacement).into_owned()
        };
    }
    out
}

// ---------------------------------------------------------------------------
// Spoken numbered lists: "number one … number two …" → "1. …\n2. …"

/// Words that introduce a list item when followed by its number.
const LIST_MARKERS: &[&str] = &["number", "point", "item", "step"];

fn list_number(word: &str) -> Option<u64> {
    let w = word.to_lowercase();
    small_number(&w).filter(|n| (1..=10).contains(n)).or_else(|| w.parse::<u64>().ok().filter(|n| (1..=10).contains(n)))
}

fn numbered_lists(s: &str) -> String {
    let tokens: Vec<&str> = s.split(' ').filter(|t| !t.is_empty()).collect();
    // Markers must count up from 1 ("number one", "number two", …).
    let mut markers: Vec<usize> = Vec::new();
    for i in 0..tokens.len().saturating_sub(1) {
        let marker = split_punct(tokens[i]).1.to_lowercase();
        if LIST_MARKERS.contains(&marker.as_str())
            && list_number(split_punct(tokens[i + 1]).1) == Some(markers.len() as u64 + 1)
        {
            markers.push(i);
        }
    }
    if markers.len() < 2 {
        return s.to_string();
    }

    let mut lines: Vec<String> = Vec::new();
    let intro = tokens[..markers[0]].join(" ");
    let intro = intro.trim_end_matches([' ', '.', ',', ':', ';']);
    if !intro.is_empty() {
        lines.push(format!("{intro}:"));
    }
    for (n, &start) in markers.iter().enumerate() {
        let end = markers.get(n + 1).copied().unwrap_or(tokens.len());
        let mut item: Vec<&str> = tokens[start + 2..end].to_vec();
        // "Number one point. We have to…": drop a dangling "point".
        if item.first().is_some_and(|w| split_punct(w).1.eq_ignore_ascii_case("point") && split_punct(w).2.contains('.')) {
            item.remove(0);
        }
        let text = item.join(" ");
        let text = text.trim_matches([' ', ',', ':', ';', '-']);
        if text.is_empty() {
            continue;
        }
        let mut chars = text.chars();
        let first: String = chars.next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
        lines.push(format!("{}. {first}{}", n + 1, chars.as_str()));
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Number words → digits

fn small_number(w: &str) -> Option<u64> {
    let v = match w {
        "zero" | "oh" => 0,
        "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        "ten" => 10,
        "eleven" => 11,
        "twelve" => 12,
        "thirteen" => 13,
        "fourteen" => 14,
        "fifteen" => 15,
        "sixteen" => 16,
        "seventeen" => 17,
        "eighteen" => 18,
        "nineteen" => 19,
        _ => return None,
    };
    Some(v)
}

fn tens_number(w: &str) -> Option<u64> {
    let v = match w {
        "twenty" => 20,
        "thirty" => 30,
        "forty" => 40,
        "fifty" => 50,
        "sixty" => 60,
        "seventy" => 70,
        "eighty" => 80,
        "ninety" => 90,
        _ => return None,
    };
    Some(v)
}

fn scale_number(w: &str) -> Option<u64> {
    let v = match w {
        "hundred" => 100,
        "thousand" => 1_000,
        "lakh" | "lakhs" | "lac" => 100_000,
        "million" => 1_000_000,
        "crore" | "crores" => 10_000_000,
        "billion" => 1_000_000_000,
        _ => return None,
    };
    Some(v)
}

fn is_number_word(w: &str) -> bool {
    // "oh" only counts inside a run (e.g. years), never on its own.
    (small_number(w).is_some() && w != "oh") || tens_number(w).is_some() || scale_number(w).is_some()
}

/// Words that make even a single small number read better as digits.
const UNIT_WORDS: &[&str] = &[
    "percent", "per", "%", "rupees", "rupee", "rs", "dollars", "dollar", "euros", "euro", "am", "pm", "a.m.", "p.m.",
    "kg", "kgs", "km", "kms", "gb", "mb", "minutes", "hours", "seconds", "o'clock",
];

/// Parses number words like ["twenty", "five", "thousand"] (no "and").
fn parse_number(words: &[&str]) -> Option<u64> {
    // A run of single digits ("nine eight seven six") is a digit string.
    if words.len() >= 2 && words.iter().all(|w| small_number(w).is_some_and(|v| v < 10)) {
        return words.iter().map(|w| small_number(w).unwrap().to_string()).collect::<String>().parse().ok();
    }
    if let Some(year) = parse_year(words) {
        return Some(year);
    }
    let (mut total, mut current) = (0u64, 0u64);
    let mut last_was_unit = false;
    for w in words {
        if let Some(v) = small_number(w) {
            // "twenty five" adds; "five six" doesn't make sense as one number.
            if last_was_unit || (current % 10 != 0 && current % 100 < 20 && current > 0) {
                return None;
            }
            current += v;
            last_was_unit = v < 20;
        } else if let Some(v) = tens_number(w) {
            if current % 100 != 0 {
                return None;
            }
            current += v;
            last_was_unit = false;
        } else if let Some(v) = scale_number(w) {
            if v == 100 {
                current = current.max(1) * 100;
            } else {
                total += current.max(1) * v;
                current = 0;
            }
            last_was_unit = false;
        } else {
            return None;
        }
    }
    Some(total + current)
}

/// "twenty twenty six" → 2026, "nineteen ninety nine" → 1999.
fn parse_year(words: &[&str]) -> Option<u64> {
    if words.len() < 2 || words.len() > 4 || words.iter().any(|w| scale_number(w).is_some()) {
        return None;
    }
    let two_digit = |ws: &[&str]| -> Option<u64> {
        match ws {
            [a] => small_number(a).filter(|v| *v >= 10).or_else(|| tens_number(a)),
            ["oh", b] => small_number(b).filter(|v| *v < 10),
            [a, b] => Some(tens_number(a)? + small_number(b).filter(|v| *v < 10 && *v > 0)?),
            _ => None,
        }
    };
    (1..words.len()).find_map(|k| {
        let century = two_digit(&words[..k]).filter(|c| (11..=20).contains(c))?;
        let rest = two_digit(&words[k..])?;
        Some(century * 100 + rest)
    })
}

fn format_number(n: u64) -> String {
    let digits = n.to_string();
    if n < 10_000 {
        return digits;
    }
    // Thousands separators for big numbers, like Parakeet's own output.
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn number_words(s: &str) -> String {
    // Expand hyphenated numbers ("twenty-five") into separate words first.
    let tokens: Vec<String> = s
        .split(' ')
        .flat_map(|t| {
            let (_, word, _) = split_punct(t);
            let lower = word.to_lowercase();
            if lower.contains('-') && lower.split('-').all(is_number_word) {
                let (lead, _, trail) = split_punct(t);
                let parts: Vec<&str> = word.split('-').collect();
                let n = parts.len();
                parts
                    .into_iter()
                    .enumerate()
                    .map(|(i, p)| format!("{}{}{}", if i == 0 { lead } else { "" }, p, if i == n - 1 { trail } else { "" }))
                    .collect::<Vec<_>>()
            } else {
                vec![t.to_string()]
            }
        })
        .collect();

    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let (lead, word, _) = split_punct(&tokens[i]);
        let lower = word.to_lowercase();
        if !is_number_word(&lower) || scale_number(&lower).is_some() || !lead.is_empty() {
            out.push(tokens[i].clone());
            i += 1;
            continue;
        }
        // Collect the run: number words, inner "and"/"oh", "point" decimals.
        let mut words: Vec<String> = Vec::new();
        let mut decimals = String::new();
        let mut j = i;
        let mut trail = String::new();
        while j < tokens.len() {
            let (lead_j, w, t) = split_punct(&tokens[j]);
            let w = w.to_lowercase();
            if !lead_j.is_empty() && j > i {
                break;
            }
            let next_is_number = tokens
                .get(j + 1)
                .map(|n| is_number_word(&split_punct(n).1.to_lowercase()))
                .unwrap_or(false);
            if is_number_word(&w) || (w == "oh" && j > i) {
                words.push(w);
            } else if (w == "and" || w == "point") && next_is_number && t.is_empty() && !words.is_empty() {
                if w == "point" {
                    // Digits after "point" are read one by one.
                    let mut k = j + 1;
                    while let Some(tok) = tokens.get(k) {
                        let (_, dw, dt) = split_punct(tok);
                        match small_number(&dw.to_lowercase()).filter(|v| *v < 10) {
                            Some(d) => decimals.push_str(&d.to_string()),
                            None => break,
                        }
                        k += 1;
                        if !dt.is_empty() {
                            trail = dt.to_string();
                            break;
                        }
                    }
                    j = k;
                    break;
                }
                j += 1;
                continue;
            } else {
                break;
            }
            j += 1;
            if !t.is_empty() {
                trail = t.to_string();
                break;
            }
        }

        let refs: Vec<&str> = words.iter().map(String::as_str).collect();
        let next_word = tokens.get(j).map(|n| split_punct(n).1.to_lowercase()).unwrap_or_default();
        let prev_word = out.last().map(|p| split_punct(p).1.to_lowercase()).unwrap_or_default();
        let value = parse_number(&refs);
        let worth_converting = |v: u64| {
            v >= 10
                || refs.len() > 1
                || !decimals.is_empty()
                || UNIT_WORDS.contains(&next_word.as_str())
                || ["rs", "inr", "₹", "$"].contains(&prev_word.as_str())
        };
        match value {
            Some(v) if worth_converting(v) => {
                let mut text = if refs.len() >= 2 && refs.iter().all(|w| small_number(w).is_some_and(|d| d < 10)) {
                    // Keep leading zeros in digit strings.
                    refs.iter().map(|w| small_number(w).unwrap().to_string()).collect()
                } else if parse_year(&refs).is_some() {
                    v.to_string()
                } else {
                    format_number(v)
                };
                if !decimals.is_empty() {
                    text = format!("{text}.{decimals}");
                }
                out.push(format!("{lead}{text}{trail}"));
                i = j.max(i + 1);
            }
            _ => {
                out.push(tokens[i].clone());
                i += 1;
            }
        }
    }
    out.join(" ")
}

// ---------------------------------------------------------------------------
// Spelled-out codes: "T 5 2 W A" → "T52WA"

fn spelled_codes(s: &str) -> String {
    let tokens: Vec<&str> = s.split(' ').collect();
    let is_piece = |t: &str| {
        let (lead, w, _) = split_punct(t);
        lead.is_empty()
            && !w.is_empty()
            && ((w.len() == 1 && w.chars().all(|c| c.is_ascii_uppercase())) || w.chars().all(|c| c.is_ascii_digit()))
    };
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let mut j = i;
        while j < tokens.len() && is_piece(tokens[j]) {
            let (_, _, trail) = split_punct(tokens[j]);
            j += 1;
            if !trail.is_empty() {
                break;
            }
        }
        let run = &tokens[i..j];
        let letters = run.iter().filter(|t| split_punct(t).1.chars().all(|c| c.is_ascii_alphabetic())).count();
        let digits = run.len() - letters;
        if run.len() >= 3 && letters >= 1 && digits >= 1 {
            let code: String = run.iter().map(|t| split_punct(t).1).collect();
            let trail = split_punct(run[run.len() - 1]).2;
            out.push(format!("{code}{trail}"));
            i = j;
        } else {
            out.push(tokens[i].to_string());
            i += 1;
        }
    }
    out.join(" ")
}

// ---------------------------------------------------------------------------
// Times: "330 p.m." → "3:30 PM", "5 pm" → "5 PM"

static TIME: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\b(\d{1,2})(?::?(\d{2}))?\s*(a|p)\.?\s?m\b\.?"));

fn times(s: &str) -> String {
    TIME.replace_all(s, |c: &Captures| {
        let whole = c.get(0).unwrap();
        let hour: u32 = c[1].parse().unwrap_or(99);
        let minutes = c.get(2).map(|m| m.as_str().parse::<u32>().unwrap_or(99));
        if !(1..=12).contains(&hour) || minutes.is_some_and(|m| m >= 60) {
            return whole.as_str().to_string();
        }
        let meridiem = if c[3].eq_ignore_ascii_case("a") { "AM" } else { "PM" };
        let time = match minutes {
            Some(m) => format!("{hour}:{m:02} {meridiem}"),
            None => format!("{hour} {meridiem}"),
        };
        // "p.m." at the end of a sentence was also the full stop.
        let rest = &s[whole.end()..];
        let ends_sentence = whole.as_str().ends_with('.')
            && (rest.is_empty() || rest.starts_with(['\n']) || rest.trim_start().starts_with(|ch: char| ch.is_uppercase()));
        if ends_sentence { format!("{time}.") } else { time }
    })
    .into_owned()
}

// ---------------------------------------------------------------------------
// Money and percentages

static MONEY: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    let amount = r"(\d[\d,]*(?:\.\d+)?)";
    let size = r"(?:\s+(lakhs?|crores?|thousand|million|billion))?";
    vec![
        (re(&format!(r"(?i)\b(?:rs\.?|inr)\s*{amount}{size}")), "₹"),
        (re(&format!(r"(?i)\b{amount}{size}\s+(?:rupees?|rs\b\.?)")), "₹"),
        (re(&format!(r"(?i)\b{amount}{size}\s+(?:dollars?|usd\b)")), "$"),
        (re(&format!(r"(?i)\b{amount}{size}\s+(?:euros?)")), "€"),
    ]
});

static PERCENT: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\b(\d[\d,]*(?:\.\d+)?)\s*(?:per\s?cent|percentage)\b"));

fn money_and_percent(s: &str) -> String {
    let mut out = s.to_string();
    for (pattern, symbol) in MONEY.iter() {
        out = pattern
            .replace_all(&out, |c: &Captures| match c.get(2) {
                Some(size) => format!("{symbol}{} {}", &c[1], size.as_str().to_lowercase()),
                None => format!("{symbol}{}", &c[1]),
            })
            .into_owned();
    }
    PERCENT.replace_all(&out, "$1%").into_owned()
}

// ---------------------------------------------------------------------------
// Emails and web addresses

static DOT_WORD: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\b([a-z0-9-]+)\s+dot\s+(com|in|org|net|io|ai|co|dev|app|edu|gov|me|info)\b"));
static LOCAL_DOTS: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\b([a-z0-9]+(?:\s+dot\s+[a-z0-9]+)+)(\s+at\s+[a-z0-9-]+\.[a-z])"));
static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)\b([a-z0-9][a-z0-9._%+-]*)\s+at\s+([a-z0-9-]+(?:\.[a-z0-9-]+)*\.(?:com|in|org|net|io|ai|co|dev|app|edu|gov|me|info))\b")
});
const MAIL_PROVIDERS: &[&str] = &[
    "gmail", "googlemail", "yahoo", "outlook", "hotmail", "live", "icloud", "proton", "protonmail", "rediffmail",
    "zoho", "aol", "msn",
];

fn emails_and_domains(s: &str) -> String {
    let mut out = s.to_string();
    // "gmail dot com" → "gmail.com" (repeat for "co dot in").
    for _ in 0..2 {
        out = DOT_WORD.replace_all(&out, "$1.$2").into_owned();
    }
    out = LOCAL_DOTS
        .replace_all(&out, |c: &Captures| {
            let local = c[1].split_whitespace().filter(|w| !w.eq_ignore_ascii_case("dot")).collect::<Vec<_>>().join(".");
            format!("{local}{}", &c[2])
        })
        .into_owned();
    EMAIL
        .replace_all(&out, |c: &Captures| {
            let (local, domain) = (&c[1], &c[2]);
            let provider = domain.split('.').next().unwrap_or("").to_lowercase();
            // "meet at google.com" isn't an email; "john.smith at gmail.com" is.
            let looks_like_email = MAIL_PROVIDERS.contains(&provider.as_str())
                || local.contains(['.', '_', '+'])
                || local.chars().any(|ch| ch.is_ascii_digit());
            if looks_like_email {
                format!("{}@{}", local.to_lowercase(), domain.to_lowercase())
            } else {
                c[0].to_string()
            }
        })
        .into_owned()
}

// ---------------------------------------------------------------------------
// Phone numbers: ten digits, optionally +91, grouped as "98765 43210"

static PHONE: LazyLock<Regex> = LazyLock::new(|| re(r"(\+?91[\s-]?)?\b(\d[\d -]{8,14}\d)\b"));

fn phone_numbers(s: &str) -> String {
    PHONE
        .replace_all(s, |c: &Captures| {
            let digits: String = c[2].chars().filter(char::is_ascii_digit).collect();
            let has_separator = c[2].contains([' ', '-']);
            if digits.len() == 10 && (has_separator || c.get(1).is_some() || c[2].len() == 10) {
                let prefix = if c.get(1).is_some() { "+91 " } else { "" };
                format!("{prefix}{} {}", &digits[..5], &digits[5..])
            } else {
                c[0].to_string()
            }
        })
        .into_owned()
}

// ---------------------------------------------------------------------------
// Spacing and capitalization

static SPACE_BEFORE_PUNCT: LazyLock<Regex> = LazyLock::new(|| re(r"[ \t]+([,.;:!?])"));
static DOUBLE_PUNCT: LazyLock<Regex> = LazyLock::new(|| re(r"([,.;:!?])[,.]+"));
static MULTI_SPACE: LazyLock<Regex> = LazyLock::new(|| re(r"[ \t]{2,}"));
static SPACE_AROUND_NEWLINE: LazyLock<Regex> = LazyLock::new(|| re(r"[ \t]*\n[ \t]*"));
static LEADING_PUNCT: LazyLock<Regex> = LazyLock::new(|| re(r"^[\s,.;:]+"));

// ---------------------------------------------------------------------------
// Email layout

static GREETING: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)^((?:hi|hello|hey|dear|good (?:morning|afternoon|evening))(?:\s+[^\s,.!:\n]+){0,3})[,.!:]?\s+(\S)")
});
static SIGN_OFF: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)(^|[.!?]\s+|\n\s*)((?:thanks|thank you|many thanks|regards|best regards|kind regards|warm regards|best|cheers|sincerely)\b[^\n]*)$")
});

/// Lays out an email: the greeting on its own line, a blank line, the body,
/// a blank line, then the sign-off with the name on the next line. Works on
/// the AI's output and on plain rules output alike; text that already has
/// a greeting line is left as is.
pub fn email_layout(s: &str) -> String {
    let mut out = s.trim().to_string();
    if !out.lines().next().is_some_and(|l| GREETING.is_match(l) && l.trim_end().ends_with(',') && l.split_whitespace().count() <= 4) {
        out = GREETING
            .replace(&out, |c: &Captures| format!("{},\n\n{}", &c[1], c[2].to_uppercase()))
            .into_owned();
    }
    if let Some(c) = SIGN_OFF.captures(&out) {
        let whole = c.get(0).unwrap();
        let sign_off = c[2].trim().trim_end_matches('.');
        // "Thanks, Krishna" → "Thanks,\nKrishna"
        let formatted = match sign_off.split_once(',') {
            Some((phrase, name)) if !name.trim().is_empty() && name.split_whitespace().count() <= 3 => {
                format!("{},\n{}", phrase.trim(), name.trim())
            }
            _ => sign_off.to_string(),
        };
        let body = format!("{}{}", &out[..whole.start()], c[1].trim_end());
        out = if body.trim().is_empty() { formatted } else { format!("{}\n\n{formatted}", body.trim_end()) };
    }
    out
}

static TRAILING_SPACES: LazyLock<Regex> = LazyLock::new(|| re(r"[ \t]+\n"));
static EXTRA_BLANK_LINES: LazyLock<Regex> = LazyLock::new(|| re(r"\n{3,}"));

/// Removes spaces at line ends (the AI adds Markdown-style ones) and keeps
/// at most one blank line between paragraphs.
pub fn tidy_lines(s: &str) -> String {
    let out = TRAILING_SPACES.replace_all(s.trim(), "\n");
    EXTRA_BLANK_LINES.replace_all(&out, "\n\n").into_owned()
}

fn tidy(s: &str) -> String {
    let mut out = SPACE_AROUND_NEWLINE.replace_all(s, "\n").into_owned();
    out = SPACE_BEFORE_PUNCT.replace_all(&out, "$1").into_owned();
    out = DOUBLE_PUNCT
        .replace_all(&out, |c: &Captures| {
            // Prefer the stronger mark: "word,." → "word."
            let marks = &c[0];
            if marks.contains(['.', '!', '?']) && !c[1].contains(['!', '?']) { ".".into() } else { c[1].to_string() }
        })
        .into_owned();
    out = MULTI_SPACE.replace_all(&out, " ").into_owned();
    out = LEADING_PUNCT.replace(&out, "").into_owned();
    capitalize_sentences(out.trim())
}

/// Uppercases the first letter of the text, of each line, and after . ! ?
/// (leaving emails and web addresses alone).
fn capitalize_sentences(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut capitalize_next = true;
    let mut prev = ' ';
    for (i, c) in s.char_indices() {
        if capitalize_next && c.is_alphabetic() {
            let word_end = s[i..].find(char::is_whitespace).map(|e| i + e).unwrap_or(s.len());
            let word = &s[i..word_end];
            if word.contains(['@', '.']) && !word.ends_with('.') {
                out.push(c);
            } else {
                out.extend(c.to_uppercase());
            }
            capitalize_next = false;
        } else {
            out.push(c);
            if c == '\n' || (prev.is_ascii_punctuation() && matches!(prev, '.' | '!' | '?') && c.is_whitespace()) {
                capitalize_next = true;
            } else if !c.is_whitespace() {
                capitalize_next = false;
            }
        }
        if c == '\n' {
            capitalize_next = true;
        }
        prev = c;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::apply;

    #[test]
    fn real_parakeet_outputs() {
        assert_eq!(apply("Let's meet on 5 March 2026 at 330 p.m."), "Let's meet on 5 March 2026 at 3:30 PM.");
        assert_eq!(apply("Send it to John.smith at gmail.com"), "Send it to john.smith@gmail.com");
        assert_eq!(apply("Call me at 987-6543210."), "Call me at 98765 43210.");
        assert_eq!(
            apply("Um, so I think, uh, we should go at five. No wait, at six"),
            "So I think, we should go at five. No wait, at six"
        );
        assert_eq!(
            apply("Buy milk, eggs and bread full stop new line call mom."),
            "Buy milk, eggs and bread.\nCall mom."
        );
        assert_eq!(apply("My vehicle number is T52WA"), "My vehicle number is T52WA");
        assert_eq!(apply("Revenue grew by 12.5% this quarter."), "Revenue grew by 12.5% this quarter.");
    }

    #[test]
    fn spoken_numbered_lists() {
        // Real dictation from the user.
        assert_eq!(
            apply("I want you to note down two points. Number one point. We have to go to the party. Number two, we have to go in the car."),
            "I want you to note down two points:\n1. We have to go to the party.\n2. We have to go in the car."
        );
        assert_eq!(
            apply("Steps: step 1 open the app, step 2 sign in, step 3 click dictate."),
            "Steps:\n1. Open the app\n2. Sign in\n3. Click dictate."
        );
        // A lone "number one" isn't a list; numbers out of order aren't either.
        assert_eq!(apply("He is the number one player."), "He is the number one player.");
        assert_eq!(apply("Point two and point one."), "Point two and point one.");
    }

    #[test]
    fn email_layout_from_dictation() {
        use super::{email_layout, tidy_lines};
        // Rules output for a dictated email with a list (no AI step).
        assert_eq!(
            email_layout(&apply("Hi team, quick update. Number one, the app now records instantly. Number two, whispers work with the hybrid model. Thanks, Krishna")),
            "Hi team,\n\nQuick update:\n1. The app now records instantly.\n2. Whispers work with the hybrid model.\n\nThanks,\nKrishna"
        );
        // Plain sentence email.
        assert_eq!(
            email_layout("Hi Ajay, the demo is ready. Can we review it on Monday? Regards, Krishna."),
            "Hi Ajay,\n\nThe demo is ready. Can we review it on Monday?\n\nRegards,\nKrishna"
        );
        // Already laid out by the AI: unchanged apart from tidying.
        let ai = "Hi Ajay,  \n\nHope you are doing well.\n\nThanks,  \nKrishna";
        assert_eq!(tidy_lines(&email_layout(ai)), "Hi Ajay,\n\nHope you are doing well.\n\nThanks,\nKrishna");
        // No greeting or sign-off: nothing to do.
        assert_eq!(email_layout("The report is attached."), "The report is attached.");
    }

    #[test]
    fn hallucination_loops() {
        // Real output on quiet audio.
        assert_eq!(
            apply("And I would love to get a little bit of a little bit of a little bit of a little bit of a case."),
            "And I would love to get a little bit of a case."
        );
        assert_eq!(apply("I really really want this"), "I really really want this");
        assert_eq!(apply("no no no no no stop"), "No stop");
        assert_eq!(apply("go go go"), "Go go go");
    }

    #[test]
    fn spelled_codes() {
        assert_eq!(apply("My vehicle number is T 5 2 W A."), "My vehicle number is T52WA.");
        assert_eq!(apply("Booking ref A B 12 C"), "Booking ref AB12C");
        assert_eq!(apply("I have 3 apples"), "I have 3 apples");
    }

    #[test]
    fn number_words() {
        assert_eq!(apply("it costs twenty five thousand rupees"), "It costs ₹25,000");
        assert_eq!(apply("twelve point five percent"), "12.5%");
        assert_eq!(apply("call nine eight seven six five four three two one zero"), "Call 98765 43210");
        assert_eq!(apply("in twenty twenty six"), "In 2026");
        assert_eq!(apply("nineteen ninety nine was fun"), "1999 was fun");
        assert_eq!(apply("one hundred and five people"), "105 people");
        assert_eq!(apply("I have two dogs"), "I have two dogs");
        assert_eq!(apply("five kg of rice"), "5 kg of rice");
        assert_eq!(apply("twenty-five people"), "25 people");
    }

    #[test]
    fn times_and_money() {
        assert_eq!(apply("see you at 5 pm"), "See you at 5 PM");
        assert_eq!(apply("at 11:45 a.m. sharp"), "At 11:45 AM sharp");
        assert_eq!(apply("pay rs 500 now"), "Pay ₹500 now");
        assert_eq!(apply("about 5 lakh rupees"), "About ₹5 lakh");
        assert_eq!(apply("it was 20 dollars"), "It was $20");
    }

    #[test]
    fn emails_and_domains() {
        assert_eq!(apply("mail me at priya dot k at gmail dot com"), "Mail me at priya.k@gmail.com");
        assert_eq!(apply("visit srutam dot app"), "Visit srutam.app");
        assert_eq!(apply("meet at google.com office"), "Meet at google.com office");
    }

    #[test]
    fn punctuation_commands() {
        assert_eq!(apply("is it ready question mark"), "Is it ready?");
        assert_eq!(apply("first point new paragraph second point"), "First point\n\nSecond point");
        assert_eq!(apply("apples comma oranges"), "Apples, oranges");
        assert_eq!(apply("put a comma here"), "Put a comma here");
    }
}
