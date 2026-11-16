//! Small shared form validators (messages in Romanian).

pub fn validate_admin_username(u: &str) -> Result<(), String> {
    let ok_len = (3..=32).contains(&u.chars().count());
    let ok_chars = u
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
    if ok_len && ok_chars {
        Ok(())
    } else {
        Err("Numele de utilizator trebuie să aibă 3–32 de caractere: litere mici, cifre, „.”, „_” sau „-”.".into())
    }
}

pub fn validate_new_password(pw: &str, confirm: &str) -> Result<(), String> {
    if pw.chars().count() < 8 {
        return Err("Parola trebuie să aibă cel puțin 8 caractere.".into());
    }
    if pw.len() > 256 {
        return Err("Parola este prea lungă.".into());
    }
    if pw != confirm {
        return Err("Parolele nu coincid.".into());
    }
    Ok(())
}
