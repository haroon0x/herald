//! Deterministic tweet text quality scoring.
//!
//! Ported from Zyquor (`packages/generation/src/quality.ts`) so the autopost
//! lane has the same floor the author calibrated. The scorer only reads text;
//! it never rewrites it. Herald never rewrites an approved body.
//!
//! Rust `regex` word boundaries are Unicode-aware, which is a strict
//! improvement over the ASCII-biased JS behaviour for non-English posts.

use std::sync::LazyLock;

use regex::Regex;

const AI_SLOP_PHRASES: &[&str] = &[
    "let's dive in",
    "let us dive in",
    "dive in",
    "delve",
    "delve into",
    "tapestry",
    "it's important to note",
    "in conclusion",
    "in today's",
    "in this day and age",
    "embark",
    "navigate the landscape",
    "navigate the complexities",
    "navigate the world",
    "game-changer",
    "game changer",
    "unlock the power",
    "unlock your",
    "robust solution",
    "leverage",
    "leverages",
    "synergy",
    "synergies",
    "cutting-edge",
    "cutting edge",
    "revolutionize",
    "revolutionary",
    "streamline",
    "streamlines",
    "holistic",
    "paradigm",
    "paradigm shift",
    "unleash the power",
    "unleash",
    "world-class",
    "world class",
    "seamless",
    "seamlessly",
    "elevate your",
    "empower",
    "harness the power",
    "harness the",
    "foster",
    "propel",
    "redefine",
    "in essence",
    "firstly",
    "secondly",
    "thirdly",
    "lastly",
    "moving forward",
    "at the end of the day",
    "when it comes to",
    "in the realm of",
    "in the world of",
    "a myriad of",
    "myriad of",
    "plethora of",
    "in this article",
    "this article will",
    "this guide will",
    "hope this helps",
    "as an ai",
    "as a language model",
    "as a large language model",
    "i cannot",
    "i'm sorry",
    "i am sorry",
    "i don't have",
    "feel free to",
    "don't hesitate to",
    "in summary",
    "to summarize",
    "ultimately",
    "ensure",
    "in order to",
    "in order for",
];

fn join_alternation(words: &[&str]) -> String {
    words
        .iter()
        .map(|word| regex::escape(word))
        .collect::<Vec<_>>()
        .join("|")
}

static INTERNAL_LEAK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:SourcePackets?|source packets?|Compact input|candidate tweet|draft candidate|strict JSON|(?:candidate|draft|source) metadata|grounded facts?|source context|zyquor|I distilled)\b|\bdistill(ed|ing)?\b.*\b(?:article|announcement|content)\b|</?source-(?:title|url)").expect("internal leak pattern must compile")
});
static INCOMPLETE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\.\.\.|…)\s*$|\b(?:read more|continue reading|more below)\b|[:;,]\s*$")
        .expect("incomplete pattern must compile")
});
static AI_SLOP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)\b(?:{})\b",
        join_alternation(AI_SLOP_PHRASES)
    ))
    .expect("ai slop pattern must compile")
});
static GENERIC_SLOP: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)\bgame[- ]chang(?:er|ing)\b",
        r"(?i)\bunlock\b",
        r"(?i)\bleverage\b",
        r"(?i)\bsynergy\b",
        r"(?i)\bin today's (?:fast-paced )?world\b",
        r"(?i)\bnow more than ever\b",
        r"(?i)\bas an ai\b",
        r"(?i)\blet that sink in\b",
        r"(?i)\bread that again\b",
        r"(?i)\bhere'?s the (?:thing|kicker)\b",
        r"(?i)\bthe best part\?",
        r"(?i)\bhot take:\b",
        r"(?i)\bunpopular opinion\b",
        r"(?i)\b(?:nobody|no one) talks about\b",
        r"(?i)\bdelve\b",
        r"(?i)\bmind.?blowing\b",
        r"(?i)\bthis changes everything\b",
        r"(?i)\bthe future is here\b",
        r"(?i)\bhere'?s why\b",
        r"(?i)\bwhy this matters\b",
        r"(?i)\b(?:more possibilities|new possibilities)\b",
        r"(?i)\bit'?s not [^.!?\n]{1,60}, it'?s\b",
        r"(?i)\b(?:is|are|was|were)n'?t just [^.!?\n]{1,60}, (?:it|they)'?(?:s|re)\b",
        r"(?i)\b(?:wild|huge|insane)\.$",
    ]
    .into_iter()
    .map(|pattern| Regex::new(pattern).expect("generic slop pattern must compile"))
    .collect()
});
static LINKEDIN_VOICE: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)\b(?:thrilled|excited|honored|humbled|proud|grateful|blessed|journey|exciting news|big news|i'?m excited to announce|we are pleased to announce)\b",
        r"(?i)\bleverage\b",
        r"(?i)\bunlocking\b",
        r"(?i)\bthought leadership\b",
        r"(?i)\bkey takeaways\b",
    ]
    .into_iter()
    .map(|pattern| Regex::new(pattern).expect("linkedin voice pattern must compile"))
    .collect()
});
static WEAK_ENDING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:\b(?:etc\.?|so|and|but|or|thoughts\??|agree\??|what do you think\??|does this resonate\??|let me know(?: what you think)?[.!?]?)|\.\.\.|…)\s*$")
        .expect("weak ending pattern must compile")
});
static EM_DASH_OVERUSE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:[—–][^—–]*){2,}").expect("em dash overuse pattern must compile")
});
static WEAK_OPENER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:interesting|(?:a|quick|friendly) reminder|this shows|fun fact|breaking\b|just in|did you know|imagine|picture this|in a world)")
        .expect("weak opener pattern must compile")
});
static ADJECTIVE_BLOAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:exciting|thrilling|groundbreaking|revolutionary|disruptive|innovative|cutting-edge|state-of-the-art|powerful|robust|seamless|effortless|comprehensive|holistic|impactful|strategic|dynamic|scalable)\b")
        .expect("adjective bloat pattern must compile")
});
static PAYOFF: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)\b(?:if|unless|until|whenever)\b",
        r"(?i)\bwhen\b",
        r"(?i)\b(?:which|that) means\b",
        r"(?i)\bbecause\b",
        r"(?i)\bbut\b",
        r"(?i)\bso (?:you|they|we|i|the|it)\b",
        r"(?i)\bstopped? \w+ing\b",
        r"(?i)\b(?:instead of|rather than|not worth|only|by removing|the (?:cost|tradeoff|catch|limit|constraint|difference|point|question) is)\b",
        r"(?i)\btrade[- ]?offs?\b",
        r"(?i)\b(?:gave up|replaced|after removing|without)\b",
        r"(?i)\bturns? .{3,40} into\b",
        r"(?i)\bmatters (?:most )?(?:to|when|because)\b",
    ]
    .into_iter()
    .map(|pattern| Regex::new(pattern).expect("payoff pattern must compile"))
    .collect()
});
static APHORISM: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)\bproperty of\b",
        r"(?i)\bis the one that\b",
        r"(?i)\bthe one that cannot\b",
        r"(?i)\b(?:is|are) exactly the (?:part|thing|one)\b",
        r"(?i)\bdivergent\b",
        r"(?i)\bnot [a-z][\w ']{0,20}, it[ ']s\b",
        r"(?i)\b(?:different|divergent|moral) (?:principles or readings|grounds for|readings of the situation)\b",
        r"(?i)\bis (?:not |never )?[a-z][\w '-]{0,20}, (?:and )?[a-z][\w '-]{0,30} is (?:not |never )?[a-z]",
    ]
    .into_iter()
    .map(|pattern| Regex::new(pattern).expect("aphorism pattern must compile"))
    .collect()
});
static RECAP_OPENER: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)^(?:a |an )?new\b",
        r"(?i)^(?:researchers|the team|the paper|the post|the article|the release|the project|the company)\b",
        r"(?i)^\w[\w .'-]{0,60}\b(?:released|announced|published|launched|shipped|introduced|arrives?|is here|has arrived|hits (?:v?\d|stable)|reaches stable|is now (?:a )?stable)\b",
        r"(?i)\bafter years of\b",
    ]
    .into_iter()
    .map(|pattern| Regex::new(pattern).expect("recap opener pattern must compile"))
    .collect()
});
static SOURCE_ARTIFACT: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)\bverified (?:facts?|part)\b",
        r"(?i)\bknown summary\b",
        r"(?i)\bsource (?:title|name)\b",
        r"(?i)\bthis milestone\b",
        r"(?i)\b(?:workflow )?impact is\b",
    ]
    .into_iter()
    .map(|pattern| Regex::new(pattern).expect("source artifact pattern must compile"))
    .collect()
});
static SPECIFIC_NOUNS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:model|database|API|kernel|compiler|chip|GPU|CPU|LLM|agent|router|index|cache|queue|token|protocol|framework|library|repo|repository|commit|release|version|patch|benchmark|latency|throughput|dataset|embedding|vector|cluster|node|pod|container|function|service|endpoint|query|transaction|inference|training|fine-tuning|distillation|quantization|attention|transformer|diffusion|RAG|SLM|MoE|dense|sparse|search|symbol|definition|mention|text)\b")
        .expect("specific nouns pattern must compile")
});
static FIRST_PERSON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:i|we|my|our|i'm|i've)\b").expect("first person pattern must compile")
});
static FIRST_PERSON_LED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:i|we|my|our)\b").expect("first person led pattern must compile")
});
static CHANGELOG_VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bv?\d+\.\d+(?:\.\d+)?\b").expect("changelog version pattern must compile")
});
static CODE_TOKEN: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"\b[a-zA-Z_][\w$]*\([^()\n]*\)",
        r"\b[a-z]+(?:_[a-z0-9]+){1,}\b",
        r"\bself\.\w+\b",
    ]
    .into_iter()
    .map(|pattern| Regex::new(pattern).expect("code token pattern must compile"))
    .collect()
});
static URLS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https?://").expect("url pattern must compile"));
static DIGITS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[0-9]").expect("digit pattern must compile"));
static INTERROGATION_DODGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:explain|describe|tell\s+me|show\s+me|walk\s+me|can\s+you|could\s+you|would\s+you|please\s+(?:explain|describe|show|make|write|do)|what\s+(?:is|are|does)|what\s+kind\s+of|how\s+(?:do|does|would|to)|why\s+(?:do|does|is|are)|which\s+(?:one|part|api|class|tool|module)|make\s+me|write\s+me)\b")
        .expect("interrogation dodge pattern must compile")
});
static LEADING_BRACKET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[-*\[{]").expect("leading bracket pattern must compile"));

fn count_payoff_signals(text: &str) -> usize {
    // JS semantics used a negative lookahead for "when it comes to"; the
    // regex crate has no lookahead, so that one case is filtered manually.
    PAYOFF
        .iter()
        .enumerate()
        .filter(|(index, pattern)| match index {
            1 => pattern
                .find_iter(text)
                .any(|m| !text[m.start()..].starts_with("when it comes to")),
            _ => pattern.is_match(text),
        })
        .count()
}

fn count_specific_nouns(text: &str) -> usize {
    SPECIFIC_NOUNS.find_iter(text).count()
}

fn contains_any(patterns: &[Regex], text: &str) -> bool {
    patterns.iter().any(|pattern| pattern.is_match(text))
}

fn looks_like_source_recap(text: &str) -> bool {
    let trimmed = text.trim();
    if FIRST_PERSON_LED.is_match(trimmed) {
        return false;
    }
    contains_any(&RECAP_OPENER, trimmed)
}

fn looks_like_third_party_changelog(text: &str) -> bool {
    if FIRST_PERSON.is_match(text) {
        return false;
    }
    if !CHANGELOG_VERSION.is_match(text) {
        return false;
    }
    contains_any(&CODE_TOKEN, text)
}

fn contains_source_artifact(text: &str) -> bool {
    contains_any(&SOURCE_ARTIFACT, text)
}

fn contains_generic_slop(text: &str) -> bool {
    AI_SLOP.is_match(text) || contains_any(&GENERIC_SLOP, text)
}

fn contains_linkedin_voice(text: &str) -> bool {
    contains_any(&LINKEDIN_VOICE, text)
}

/// Additive tweet quality score. Starts at 3 and moves with concrete signals
/// (numbers, links, specific vocabulary, real payoff) and against recap,
/// aphorism, and slop patterns.
#[must_use]
pub fn score_tweet_quality(text: &str) -> i32 {
    let trimmed = text.trim();
    let payoff_signals = count_payoff_signals(trimmed);
    let mut score: i32 = 3;
    if DIGITS.is_match(trimmed) {
        score += 1;
    }
    if URLS.is_match(trimmed) {
        score += 1;
    }
    score += i32::try_from(count_specific_nouns(trimmed).min(3)).unwrap_or(3);
    if payoff_signals > 0 {
        score += 2;
    }
    if contains_source_artifact(trimmed) {
        score -= 3;
    }
    if looks_like_source_recap(trimmed) {
        score -= 3;
    }
    if looks_like_third_party_changelog(trimmed) {
        score -= 4;
    }
    if WEAK_OPENER.is_match(trimmed) {
        score -= 2;
    }
    if ADJECTIVE_BLOAT.is_match(trimmed) {
        score -= 2;
    }
    if contains_any(&APHORISM, trimmed) {
        score -= 3;
    }
    score
}

/// True when the text has no internal leakage, no incomplete ending, no
/// generic slop, no `LinkedIn` voice, no weak ending, no em-dash pile-up, no
/// leading markdown bracket, and no replacement characters.
#[must_use]
pub fn is_clean_tweet_text(text: &str) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty()
        && !INTERNAL_LEAK.is_match(trimmed)
        && !INCOMPLETE.is_match(trimmed)
        && !contains_generic_slop(trimmed)
        && !contains_linkedin_voice(trimmed)
        && !WEAK_ENDING.is_match(trimmed)
        && !EM_DASH_OVERUSE.is_match(trimmed)
        && !LEADING_BRACKET.is_match(trimmed)
        && !trimmed.contains('\u{FFFD}')
}

/// True when the text is clean, is not a source recap, and reaches `minimum`.
#[must_use]
pub fn passes_tweet_quality_floor(text: &str, minimum: u8) -> bool {
    is_clean_tweet_text(text)
        && !looks_like_source_recap(text)
        && score_tweet_quality(text) >= i32::from(minimum)
}

/// True when a reply opens by demanding the author explain or build their own
/// thing. Ported from the Zyquor reply screen; flagged once live on
/// 2026-10-29.
#[must_use]
pub fn is_interrogation_dodge_reply(text: &str) -> bool {
    let unquoted = text.trim().trim_start_matches(['"', '\'', '“', '”']);
    INTERROGATION_DODGE.is_match(unquoted)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLOOR_7: u8 = 7;

    #[test]
    fn concrete_post_clears_floor() {
        let text = "we replaced the postgres connection pool with a queue because p99 latency was 800ms. the tradeoff is burst throughput, and for our workload that's the right call.";
        assert!(is_clean_tweet_text(text));
        assert!(score_tweet_quality(text) >= i32::from(FLOOR_7));
        assert!(passes_tweet_quality_floor(text, FLOOR_7));
    }

    #[test]
    fn first_person_release_note_is_not_recap() {
        let text = "I shipped v2.4 of our rust compiler with 40% faster incremental builds after removing the old symbol table from the hot path. benchmark: 11 releases in a week.";
        assert!(!looks_like_source_recap(text));
        assert!(score_tweet_quality(text) >= i32::from(FLOOR_7));
    }

    #[test]
    fn third_party_changelog_is_penalized() {
        let text = "llm 0.33 released: embed_multi now accepts self.key rotation and Foo.bar() retry logic. version 0.33.0 patches the token cache.";
        assert!(looks_like_third_party_changelog(text));
        assert!(score_tweet_quality(text) < i32::from(FLOOR_7));
    }

    #[test]
    fn source_recap_is_rejected() {
        let text = "Researchers released a new benchmark for long-context retrieval across 1M token windows. The paper arrives today.";
        assert!(looks_like_source_recap(text));
        assert!(!passes_tweet_quality_floor(text, FLOOR_7));
    }

    #[test]
    fn slop_opener_is_penalized() {
        let text = "here's the thing: everyone is suddenly talking about agents, and the numbers say otherwise.";
        assert!(score_tweet_quality(text) <= 4);
    }

    #[test]
    fn ai_slop_phrase_blocks_cleaning() {
        assert!(!is_clean_tweet_text(
            "let's dive in: five fixes for your rust CI."
        ));
        assert!(!is_clean_tweet_text(
            "you can leverage this to fix your parser."
        ));
    }

    #[test]
    fn incomplete_endings_block_cleaning() {
        assert!(!is_clean_tweet_text(
            "five fixes for your CI because otherwise..."
        ));
        assert!(!is_clean_tweet_text("five fixes for your CI,"));
    }

    #[test]
    fn weak_ending_blocks_cleaning() {
        assert!(!is_clean_tweet_text(
            "we replaced postgres with a queue because latency, thoughts?"
        ));
    }

    #[test]
    fn em_dash_overuse_blocks_cleaning() {
        assert!(!is_clean_tweet_text(
            "we tried — really tried — to make it — simpler."
        ));
    }

    #[test]
    fn linkedin_voice_blocks_cleaning() {
        assert!(!is_clean_tweet_text(
            "i'm excited to announce that our team shipped a fix because the bug kept people up."
        ));
    }

    #[test]
    fn aphorism_is_penalized() {
        let text = "the constraint is not the model, it's the harness.";
        assert!(contains_any(&APHORISM, text));
        assert!(score_tweet_quality(text) < 6);
    }

    #[test]
    fn empty_and_replacement_chars_are_not_clean() {
        assert!(!is_clean_tweet_text(""));
        assert!(!is_clean_tweet_text("five fixes \u{FFFD}"));
    }

    #[test]
    fn internal_leak_blocks_cleaning() {
        assert!(!is_clean_tweet_text(
            "the candidate tweet scores 7 because the model trusted the source context."
        ));
        assert!(!is_clean_tweet_text(
            "zyquor says the draft is good, because it passed the gate."
        ));
    }

    #[test]
    fn interrogation_dodge_detection_matches_zyquor() {
        assert!(is_interrogation_dodge_reply(
            "Explain how you built that cache."
        ));
        assert!(is_interrogation_dodge_reply(
            "\"how do you handle invalidation?\""
        ));
        assert!(is_interrogation_dodge_reply("which API are you using?"));
        assert!(!is_interrogation_dodge_reply(
            "we hit the same wall with our LRU and stopped using one."
        ));
    }

    #[test]
    fn floor_zero_admits_any_clean_text() {
        let text = "Thursday works.";
        // "Thursday works." is clean per the rules; floor 0 must pass it.
        assert_eq!(score_tweet_quality(text), 3);
        assert!(passes_tweet_quality_floor(text, 0));
    }
}
