//! Printing a connection string without printing the credential.
//!
//! A `DATABASE_URL` is not a diagnostic string: on a managed database
//! its userinfo is a rotating secret, and a log line has a different
//! retention policy and a different audience than the secret store the
//! value came from. The host and database name are the parts anyone
//! debugging actually wants; the password never was. So no binary in
//! this workspace prints a `DATABASE_URL` — they print
//! [`redact_url`] of one.

/// A connection string with its password replaced by `<redacted>`,
/// keeping the scheme, the user, the host and the database — the parts
/// that are useful in a log line.
///
/// Redaction is deliberately **pessimistic**: an input this cannot
/// confidently split into `scheme://[user[:password]@]host…` is
/// replaced wholesale rather than echoed. That case is not theoretical.
/// The URLs most likely to defeat a parser are exactly the ones with
/// unencoded punctuation in the password — an unencoded `/` or `?` ends
/// the authority early, so a naive scan finds no `@`, concludes there is
/// no userinfo, and prints the secret. Losing a host name from a log
/// line is cheap; that is not.
pub fn redact_url(url: &str) -> String {
    const WHOLESALE: &str = "<redacted: DATABASE_URL did not parse>";

    let Some(scheme_end) = url.find("://") else {
        return WHOLESALE.to_string();
    };
    let authority_start = scheme_end + 3;
    let rest = &url[authority_start..];

    // The authority ends at the path. Only '/' is used as the
    // terminator, NOT '?' or '#': in a Postgres URL the query follows
    // the path (`…@host:5432/db?sslmode=require`), so treating a '?' as
    // the end here would truncate a password containing one — the exact
    // input that has to redact correctly.
    let authority = rest.split('/').next().unwrap_or(rest);

    let Some(at) = authority.rfind('@') else {
        // No userinfo in the authority — but if an '@' appears later,
        // the split above landed in the middle of a credential
        // (a password containing '/'). Cannot locate the boundary:
        // redact everything.
        return if rest.contains('@') {
            WHOLESALE.to_string()
        } else {
            url.to_string()
        };
    };

    let userinfo = &authority[..at];
    // A user name with no password carries no secret. Everything after
    // the FIRST ':' does, however many more the password contains.
    let Some(colon) = userinfo.find(':') else {
        return url.to_string();
    };

    format!(
        "{}{}<redacted>{}",
        &url[..authority_start],
        &userinfo[..=colon],
        &url[authority_start + at..],
    )
}

#[cfg(test)]
mod tests {
    use super::redact_url;

    #[test]
    fn hides_the_password_and_keeps_the_target() {
        assert_eq!(
            redact_url("postgres://mcpm:s3cret@db.rds.amazonaws.com:5432/mcpm?sslmode=require"),
            "postgres://mcpm:<redacted>@db.rds.amazonaws.com:5432/mcpm?sslmode=require",
        );
    }

    #[test]
    fn leaves_urls_without_a_credential_alone() {
        assert_eq!(
            redact_url("postgres://localhost:55432/app"),
            "postgres://localhost:55432/app",
        );
        // A port is not a password: the ':' here must not be mistaken
        // for the start of one.
        assert_eq!(redact_url("postgres://mcpm@host:5432/db"), "postgres://mcpm@host:5432/db");
    }

    #[test]
    fn survives_punctuation_rds_actually_generates() {
        // The password from the report: leading ':', and a '?' that a
        // naive authority scan would read as the start of a query.
        assert_eq!(
            redact_url("postgres://mcpm::pa?ss(x)*>@host:5432/mcpm"),
            "postgres://mcpm:<redacted>@host:5432/mcpm",
        );
        // An unencoded '@' in the password: the LAST one in the
        // authority is the real delimiter.
        assert_eq!(
            redact_url("postgres://mcpm:pa@ss@host/db"),
            "postgres://mcpm:<redacted>@host/db",
        );
    }

    #[test]
    fn redacts_wholesale_rather_than_guessing() {
        // A '/' in the password puts the delimiter past the authority.
        // Nothing here is safe to echo.
        let out = redact_url("postgres://mcpm:pa/ss@host/db");
        assert!(!out.contains("pa/ss"), "leaked the password: {out}");
        // Not a URL at all.
        let out = redact_url("mcpm:s3cret@host");
        assert!(!out.contains("s3cret"), "leaked the password: {out}");
    }
}
