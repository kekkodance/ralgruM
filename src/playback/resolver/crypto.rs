use super::*;

pub(super) fn deezer_key(track_id: &str) -> [u8; 16] {
    let hex = format!("{:x}", Md5::digest(track_id.as_bytes()));
    let bytes = hex.as_bytes();
    let mut key = [0; 16];
    for index in 0..16 {
        key[index] = bytes[index] ^ bytes[index + 16] ^ DEEZER_SECRET[index];
    }
    key
}

pub(super) fn decrypt_stripes(
    bytes: &mut [u8],
    track_id: &str,
    first_stripe: u64,
) -> Result<(), String> {
    let key = deezer_key(track_id);
    let cipher = Blowfish::new_from_slice(&key)
        .map_err(|_| "The Deezer stream key was invalid".to_string())?;
    for (offset, stripe) in bytes.chunks_mut(STRIPE_SIZE).enumerate() {
        if (first_stripe + offset as u64).is_multiple_of(3) && stripe.len() == STRIPE_SIZE {
            decrypt_cbc(&cipher, stripe);
        }
    }
    Ok(())
}

pub(super) struct DeezerStripeStream {
    cipher: Blowfish,
    pending: Vec<u8>,
    output: Vec<u8>,
    next_stripe: u64,
}

impl DeezerStripeStream {
    pub(super) fn new(track_id: &str) -> Result<Self, String> {
        let key = deezer_key(track_id);
        let cipher = Blowfish::new_from_slice(&key)
            .map_err(|_| "The Deezer stream key was invalid".to_string())?;
        Ok(Self {
            cipher,
            pending: Vec::with_capacity(STRIPE_SIZE),
            output: Vec::with_capacity(DEEZER_STREAM_WRITE_BUFFER_SIZE + STRIPE_SIZE),
            next_stripe: 0,
        })
    }

    pub(super) async fn write_chunk<W>(
        &mut self,
        mut chunk: &[u8],
        output: &mut W,
    ) -> std::io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        while !chunk.is_empty() {
            let needed = STRIPE_SIZE - self.pending.len();
            let take = needed.min(chunk.len());
            self.pending.extend_from_slice(&chunk[..take]);
            chunk = &chunk[take..];
            if self.pending.len() != STRIPE_SIZE {
                continue;
            }
            if self.next_stripe.is_multiple_of(3) {
                decrypt_cbc(&self.cipher, &mut self.pending);
            }
            self.output.extend_from_slice(&self.pending);
            self.pending.clear();
            self.next_stripe += 1;
            if self.output.len() >= DEEZER_STREAM_WRITE_BUFFER_SIZE {
                output.write_all(&self.output).await?;
                self.output.clear();
            }
        }
        Ok(())
    }

    pub(super) async fn finish<W>(&mut self, output: &mut W) -> std::io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        if !self.pending.is_empty() {
            self.output.extend_from_slice(&self.pending);
            self.pending.clear();
        }
        if !self.output.is_empty() {
            output.write_all(&self.output).await?;
            self.output.clear();
        }
        Ok(())
    }
}

pub(super) fn decrypt_cbc(cipher: &Blowfish, bytes: &mut [u8]) {
    let mut previous = [0, 1, 2, 3, 4, 5, 6, 7];
    for block in bytes.as_chunks_mut::<8>().0 {
        let encrypted = *block;
        cipher.decrypt_block(GenericArray::from_mut_slice(block));
        for index in 0..8 {
            block[index] ^= previous[index];
        }
        previous = encrypted;
    }
}
