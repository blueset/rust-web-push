use ring::{hmac, hkdf, agreement, rand, digest, aead};
use untrusted::Input;
use error::WebPushError;
use message::WebPushPayload;

pub enum ContentCoding {
    AesGcm,
    Aes128Gcm,
}

pub struct HttpEce<'a> {
    peer_public_key: &'a [u8],
    peer_secret: &'a [u8],
    coding: ContentCoding,
    rng: rand::SystemRandom,
}

impl<'a> HttpEce<'a> {
    pub fn new(coding: ContentCoding, peer_public_key: &'a [u8], peer_secret: &'a [u8]) -> Result<HttpEce<'a>, WebPushError> {
        Ok(HttpEce {
            rng: rand::SystemRandom::new(),
            peer_public_key: peer_public_key,
            peer_secret: peer_secret,
            coding: coding,
        })
    }

    pub fn encrypt(&self, content: &'a [u8]) -> Result<WebPushPayload, WebPushError> {
        if content.len() > 3800 { return Err(WebPushError::ContentTooLong) }

        let private_key        = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &self.rng)?;
        let mut public_key     = [0u8; agreement::PUBLIC_KEY_MAX_LEN];
        let public_key         = &mut public_key[..private_key.public_key_len()];
        let agr                = &agreement::ECDH_P256;
        let mut salt_bytes     = [0u8; 16];
        let peer_input         = Input::from(self.peer_public_key);

        self.rng.fill(&mut salt_bytes)?;
        private_key.compute_public_key(public_key)?;

        agreement::agree_ephemeral(private_key, agr, peer_input, WebPushError::Unspecified, |shared_secret| {
            match self.coding {
                ContentCoding::AesGcm => {
                    let mut payload = [0u8; 3818];
                    front_pad(content, &mut payload);

                    self.aes_gcm(shared_secret, public_key, &salt_bytes, &mut payload)?;

                    Ok(WebPushPayload {
                        content: payload.to_vec(),
                        public_key: public_key.to_vec(),
                        salt: salt_bytes.to_vec(),
                    })
                },
                ContentCoding::Aes128Gcm =>
                    Err(WebPushError::NotImplemented("Aes128Gcm support comes when enough browsers implement it")),
            }
        })
    }

    fn aes_gcm(&self, shared_secret: &'a [u8], as_public_key: &'a [u8], salt_bytes: &'a [u8], mut payload: &'a mut [u8])
               -> Result<(), WebPushError> {
        let salt               = hmac::SigningKey::new(&digest::SHA256, salt_bytes);
        let client_auth_secret = hmac::SigningKey::new(&digest::SHA256, self.peer_secret);

        let mut context = Vec::with_capacity(140);
        context.extend_from_slice("P-256\0".as_bytes());
        context.push((self.peer_public_key.len() >> 8) as u8);
        context.push((self.peer_public_key.len() & 0xff) as u8);
        context.extend_from_slice(self.peer_public_key);
        context.push((as_public_key.len() >> 8) as u8);
        context.push((as_public_key.len() & 0xff) as u8);
        context.extend_from_slice(as_public_key);

        let mut ikm = [0u8; 32];
        hkdf::extract_and_expand(&client_auth_secret, &shared_secret, "Content-Encoding: auth\0".as_bytes(), &mut ikm);

        let mut cek_info = Vec::with_capacity(165);
        cek_info.extend_from_slice("Content-Encoding: aesgcm\0".as_bytes());
        cek_info.extend_from_slice(&context);

        let mut content_encryption_key = [0u8; 16];
        hkdf::extract_and_expand(&salt, &ikm, &cek_info, &mut content_encryption_key);

        let mut nonce_info = Vec::with_capacity(164);
        nonce_info.extend_from_slice("Content-Encoding: nonce\0".as_bytes());
        nonce_info.extend_from_slice(&context);

        let mut nonce = [0u8; 12];
        hkdf::extract_and_expand(&salt, &ikm, &nonce_info, &mut nonce);

        let sealing_key = aead::SealingKey::new(&aead::AES_128_GCM, &content_encryption_key)?;
        aead::seal_in_place(&sealing_key, &nonce, "".as_bytes(), &mut payload, 16)?;

        Ok(())
    }

}

fn front_pad(payload: &[u8], output: &mut [u8]) {
    let payload_len = payload.len();
    let max_payload = output.len() - 2 - 16;
    let padding_size = max_payload - payload.len();

    output[0] = (padding_size >> 8) as u8;
    output[1] = (padding_size & 0xff) as u8;

    for i in 0..payload_len {
        output[padding_size + i + 2] = payload[i];
    }
}
