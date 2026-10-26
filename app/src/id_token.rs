use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::signature::{RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents};
use serde::Deserialize;

const ISSUERS: [&str; 2] = ["https://accounts.google.com", "accounts.google.com"];
const CLOCK_SKEW_SECONDS: u64 = 60;

#[derive(Deserialize)]
pub struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kid: String,
    kty: String,
    n: String,
    e: String,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: String,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    aud: String,
    exp: u64,
    hd: Option<String>,
    email: Option<String>,
    #[serde(default)]
    email_verified: bool,
}

/// Returns the account email when the token is a valid Google ID token for `client_id` from
/// an account in `domain`.
pub fn verify(
    token: &str,
    jwks: &Jwks,
    client_id: &str,
    domain: &str,
    now: u64,
) -> Result<String, String> {
    let Some((signed, signature)) = token.rsplit_once('.') else {
        return Err("The Google ID token is malformed.".into());
    };
    let Some((header, payload)) = signed.split_once('.').filter(|(_, p)| !p.contains('.')) else {
        return Err("The Google ID token is malformed.".into());
    };
    let header: Header = decode_json(header)?;
    if header.alg != "RS256" {
        return Err("The Google ID token uses an unexpected algorithm.".into());
    }
    let key = jwks
        .keys
        .iter()
        .find(|k| k.kid == header.kid && k.kty == "RSA")
        .ok_or("The Google ID token is signed by an unknown key.")?;
    RsaPublicKeyComponents {
        n: decode(&key.n)?,
        e: decode(&key.e)?,
    }
    .verify(
        &RSA_PKCS1_2048_8192_SHA256,
        signed.as_bytes(),
        &decode(signature)?,
    )
    .map_err(|_| "The Google ID token signature is not valid.")?;

    let claims: Claims = decode_json(payload)?;
    if !ISSUERS.contains(&claims.iss.as_str()) {
        return Err("The Google ID token has an unexpected issuer.".into());
    }
    if claims.aud != client_id {
        return Err("The Google ID token is for a different app.".into());
    }
    if now > claims.exp + CLOCK_SKEW_SECONDS {
        return Err("The Google ID token has expired.".into());
    }
    let email = claims.email.unwrap_or_default();
    let in_domain = match claims.hd.as_deref() {
        Some(hd) => hd.eq_ignore_ascii_case(domain),
        None => {
            claims.email_verified
                && email
                    .to_ascii_lowercase()
                    .ends_with(&format!("@{}", domain.to_ascii_lowercase()))
        }
    };
    if !in_domain {
        return Err(format!(
            "Only {domain} Google accounts can use the shared key."
        ));
    }
    if email.is_empty() {
        return Err("The Google ID token has no email address.".into());
    }
    Ok(email)
}

fn decode(part: &str) -> Result<Vec<u8>, String> {
    URL_SAFE_NO_PAD
        .decode(part)
        .map_err(|_| "The Google ID token is malformed.".to_string())
}

fn decode_json<T: for<'de> Deserialize<'de>>(part: &str) -> Result<T, String> {
    serde_json::from_slice(&decode(part)?).map_err(|_| "The Google ID token is malformed.".into())
}

#[cfg(test)]
mod tests {
    use ring::rand::SystemRandom;
    use ring::rsa::PublicKeyComponents;
    use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
    use serde_json::{Value, json};

    use super::*;

    const CLIENT: &str = "client.apps.googleusercontent.com";
    const NOW: u64 = 1_800_000_000;

    fn key_pair() -> RsaKeyPair {
        RsaKeyPair::from_pkcs8(include_bytes!("testdata/id-token-test-key.der")).unwrap()
    }

    fn jwks(kid: &str) -> Jwks {
        let public = PublicKeyComponents::<Vec<u8>>::from(key_pair().public());
        serde_json::from_value(json!({"keys": [{
            "kid": kid,
            "kty": "RSA",
            "alg": "RS256",
            "use": "sig",
            "n": URL_SAFE_NO_PAD.encode(public.n),
            "e": URL_SAFE_NO_PAD.encode(public.e),
        }]}))
        .unwrap()
    }

    fn claims() -> Value {
        json!({
            "iss": "https://accounts.google.com",
            "aud": CLIENT,
            "exp": NOW + 3600,
            "iat": NOW,
            "hd": "liatrio.com",
            "email": "dev@liatrio.com",
            "email_verified": true,
        })
    }

    fn sign(claims: &Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(json!({"alg": "RS256", "kid": "k1"}).to_string());
        let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
        let message = format!("{header}.{payload}");
        let pair = key_pair();
        let mut signature = vec![0; pair.public().modulus_len()];
        pair.sign(
            &RSA_PKCS1_SHA256,
            &SystemRandom::new(),
            message.as_bytes(),
            &mut signature,
        )
        .unwrap();
        format!("{message}.{}", URL_SAFE_NO_PAD.encode(signature))
    }

    fn check(token: &str) -> Result<String, String> {
        verify(token, &jwks("k1"), CLIENT, "liatrio.com", NOW)
    }

    #[test]
    fn accepts_workspace_account() {
        assert_eq!(check(&sign(&claims())).unwrap(), "dev@liatrio.com");
    }

    #[test]
    fn rejects_other_hosted_domain() {
        let mut claims = claims();
        claims["hd"] = json!("example.com");
        claims["email"] = json!("dev@liatrio.com");
        assert!(check(&sign(&claims)).is_err());
    }

    #[test]
    fn falls_back_to_verified_email_without_hd() {
        let mut claims = claims();
        claims.as_object_mut().unwrap().remove("hd");
        assert_eq!(check(&sign(&claims)).unwrap(), "dev@liatrio.com");
    }

    #[test]
    fn rejects_unverified_email_without_hd() {
        let mut claims = claims();
        claims.as_object_mut().unwrap().remove("hd");
        claims["email_verified"] = json!(false);
        assert!(check(&sign(&claims)).is_err());
    }

    #[test]
    fn rejects_lookalike_email_domain() {
        let mut claims = claims();
        claims.as_object_mut().unwrap().remove("hd");
        claims["email"] = json!("dev@notliatrio.com");
        assert!(check(&sign(&claims)).is_err());
    }

    #[test]
    fn rejects_gmail_account() {
        let mut claims = claims();
        claims.as_object_mut().unwrap().remove("hd");
        claims["email"] = json!("dev@gmail.com");
        assert!(check(&sign(&claims)).is_err());
    }

    #[test]
    fn rejects_expired_token() {
        let mut claims = claims();
        claims["exp"] = json!(NOW - CLOCK_SKEW_SECONDS - 1);
        assert!(check(&sign(&claims)).unwrap_err().contains("expired"));
    }

    #[test]
    fn rejects_wrong_audience() {
        let mut claims = claims();
        claims["aud"] = json!("other.apps.googleusercontent.com");
        assert!(check(&sign(&claims)).is_err());
    }

    #[test]
    fn rejects_wrong_issuer() {
        let mut claims = claims();
        claims["iss"] = json!("https://evil.example");
        assert!(check(&sign(&claims)).is_err());
    }

    #[test]
    fn rejects_bad_signature() {
        let token = sign(&claims());
        let (signed, _) = token.rsplit_once('.').unwrap();
        let forged = URL_SAFE_NO_PAD.encode(vec![7u8; 256]);
        assert!(
            check(&format!("{signed}.{forged}"))
                .unwrap_err()
                .contains("signature")
        );
    }

    #[test]
    fn rejects_tampered_payload() {
        let token = sign(&claims());
        let parts: Vec<&str> = token.split('.').collect();
        let mut claims = claims();
        claims["hd"] = json!("liatrio.com");
        claims["email"] = json!("someone@gmail.com");
        let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
        let tampered = format!("{}.{payload}.{}", parts[0], parts[2]);
        assert!(check(&tampered).unwrap_err().contains("signature"));
    }

    #[test]
    fn rejects_unknown_key() {
        let token = sign(&claims());
        assert!(verify(&token, &jwks("other"), CLIENT, "liatrio.com", NOW).is_err());
    }

    #[test]
    fn rejects_unsigned_algorithm() {
        let header = URL_SAFE_NO_PAD.encode(json!({"alg": "none", "kid": "k1"}).to_string());
        let payload = URL_SAFE_NO_PAD.encode(claims().to_string());
        assert!(check(&format!("{header}.{payload}.")).is_err());
    }
}
