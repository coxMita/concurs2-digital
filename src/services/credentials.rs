//! Random tokens, readable passwords and student usernames.

use rand::{RngExt, seq::IndexedRandom};

const TOKEN_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
/// No ambiguous characters: 0/O/o, 1/l/I/i, 5/S, 2/Z are excluded.
const PASSWORD_ALPHABET: &[u8] = b"abcdefghjkmnpqrtuvwxyz346789";

/// Cryptographically random alphanumeric token (thread RNG is a CSPRNG).
pub fn random_token(len: usize) -> String {
    let mut rng = rand::rng();
    (0..len)
        .map(|_| *TOKEN_ALPHABET.choose(&mut rng).expect("non-empty") as char)
        .collect()
}

/// Readable 8-character password without ambiguous characters,
/// always containing at least one digit.
pub fn generate_password() -> String {
    let mut rng = rand::rng();
    loop {
        let pw: String = (0..8)
            .map(|_| *PASSWORD_ALPHABET.choose(&mut rng).expect("non-empty") as char)
            .collect();
        if pw.chars().any(|c| c.is_ascii_digit()) && pw.chars().any(|c| c.is_ascii_alphabetic()) {
            return pw;
        }
    }
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    rand::rng().fill(&mut buf[..]);
    buf
}

/// Strips Romanian (and other common) diacritics and lowercases.
fn fold_char(c: char) -> Option<char> {
    let c = match c {
        'ă' | 'â' | 'Ă' | 'Â' | 'á' | 'à' | 'ä' | 'Á' | 'À' | 'Ä' => 'a',
        'î' | 'Î' | 'í' | 'ì' | 'ï' | 'Í' => 'i',
        'ș' | 'ş' | 'Ș' | 'Ş' | 'š' | 'Š' => 's',
        'ț' | 'ţ' | 'Ț' | 'Ţ' => 't',
        'é' | 'è' | 'ë' | 'É' | 'È' => 'e',
        'ó' | 'ö' | 'ő' | 'Ó' | 'Ö' => 'o',
        'ú' | 'ü' | 'ű' | 'Ú' | 'Ü' => 'u',
        'ç' | 'Ç' => 'c',
        'ž' | 'Ž' => 'z',
        c if c.is_ascii_alphanumeric() => c.to_ascii_lowercase(),
        _ => return None,
    };
    Some(c)
}

/// "Ana-Maria Popescu" -> "ana-maria.popescu" style base username:
/// words joined by '.', only [a-z0-9-] kept. Returns None if nothing usable remains.
pub fn username_base_from_name(name: &str) -> Option<String> {
    let words: Vec<String> = name
        .split_whitespace()
        .map(|w| {
            w.chars()
                .filter_map(|c| if c == '-' { Some('-') } else { fold_char(c) })
                .collect::<String>()
                .trim_matches('-')
                .to_string()
        })
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return None;
    }
    let mut base = words.join(".");
    base.truncate(40);
    Some(base.trim_end_matches(['.', '-']).to_string())
}

/// Picks a username not in `taken`: base, base.2, base.3, ...
pub fn dedupe(base: &str, taken: &std::collections::HashSet<String>) -> String {
    if !taken.contains(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}.{n}"))
        .find(|u| !taken.contains(u))
        .expect("infinite iterator")
}

/// Plans usernames for new accounts: one per name (e.g. "ana.popescu", deduplicated),
/// then `unnamed` accounts "elev001", "elev002", ... continuing after the highest
/// existing number. `taken` is updated with the new usernames.
pub fn plan_usernames(
    names: &[String],
    unnamed: usize,
    taken: &mut std::collections::HashSet<String>,
) -> Vec<(Option<String>, String)> {
    let mut out = Vec::with_capacity(names.len() + unnamed);
    for name in names {
        let base = username_base_from_name(name).unwrap_or_else(|| "elev".to_string());
        let u = dedupe(&base, taken);
        taken.insert(u.clone());
        out.push((Some(name.clone()), u));
    }
    let mut n = taken
        .iter()
        .filter_map(|u| u.strip_prefix("elev")?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    for _ in 0..unnamed {
        loop {
            n += 1;
            let u = format!("elev{n:03}");
            if taken.insert(u.clone()) {
                out.push((None, u));
                break;
            }
        }
    }
    out
}

/// Normalizes a username typed at login: trimmed, lowercase.
pub fn normalize_username(u: &str) -> String {
    u.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_are_readable() {
        for _ in 0..200 {
            let p = generate_password();
            assert_eq!(p.len(), 8);
            assert!(!p.contains(['0', 'O', 'o', '1', 'l', 'I', 'i']));
        }
    }

    #[test]
    fn usernames_from_names() {
        assert_eq!(
            username_base_from_name("  Ana  Popescu ").as_deref(),
            Some("ana.popescu")
        );
        assert_eq!(
            username_base_from_name("Ștefan Țăran-Ilie").as_deref(),
            Some("stefan.taran-ilie")
        );
        assert_eq!(username_base_from_name("!!!"), None);
        let taken = ["ana.popescu".to_string(), "ana.popescu.2".to_string()]
            .into_iter()
            .collect();
        assert_eq!(dedupe("ana.popescu", &taken), "ana.popescu.3");
    }

    #[test]
    fn plans_usernames() {
        let mut taken: std::collections::HashSet<String> =
            ["elev007".to_string(), "ana.popescu".to_string()]
                .into_iter()
                .collect();
        let names = vec!["Ana Popescu".to_string(), "Ana Popescu".to_string()];
        let plan = plan_usernames(&names, 2, &mut taken);
        let users: Vec<&str> = plan.iter().map(|p| p.1.as_str()).collect();
        assert_eq!(
            users,
            ["ana.popescu.2", "ana.popescu.3", "elev008", "elev009"]
        );
        assert_eq!(plan[0].0.as_deref(), Some("Ana Popescu"));
        assert_eq!(plan[2].0, None);
    }
}
