//! The phone's own key service of the older kind: a hardware service reached
//! over the driver the phone's hardware services talk on, which is the only
//! thing that can use a key the hardware wrapped.
//!
//! It speaks the same operations as the newer kind where the recovery needs
//! them, under the numbers its interface declares them by. Where the newer
//! kind makes a storage key usable for this start of the phone with an
//! operation of its own, this one exports it: the secure world hands back
//! the key wrapped for the hardware, which is what the storage hardware
//! takes.

use crate::token::Token;
use anyhow::{ensure, Context, Result};
use skyward_binder::driver::{Binder, Handle};
use skyward_binder::hidl::{self, Answer, Message};

/// The phone's own key service, the only thing that can use a key the
/// hardware wrapped, and the instance it is looked up by.
pub const SERVICE: &str = "android.hardware.keymaster@4.0::IKeymasterDevice";
const INSTANCE: &str = "default";
const INTERFACE: &str = SERVICE;

/// The methods the recovery calls, by the numbers the interface declares
/// them in.
const GET_HARDWARE_INFO: u32 = 1;
const GET_KEY_CHARACTERISTICS: u32 = 9;
const EXPORT_KEY: u32 = 10;
const UPGRADE_KEY: u32 = 12;
const BEGIN: u32 = 16;
const UPDATE: u32 = 17;
const FINISH: u32 = 18;
const ABORT: u32 = 19;

/// How the key service's own enums are numbered.
const FORMAT_RAW: u32 = 3;
const PURPOSE_DECRYPT: u32 = 1;
const BLOCK_MODE_GCM: u32 = 32;
const PADDING_NONE: u32 = 1;

/// The key service's way of saying the key was made under an older build of
/// the phone's own firmware and has to be renewed before it will be used.
const KEY_REQUIRES_UPGRADE: i32 = -62;

/// How long the nonce of an AES-GCM key is here.
const NONCE_BYTES: usize = 12;

/// The top of a tag says what kind of value it carries.
const TAG_KIND: i32 = -268_435_456;
const KIND_ULONG: i32 = 1_342_177_280;
const KIND_ULONG_REP: i32 = -1_610_612_736;
const KIND_DATE: i32 = 1_610_612_736;
const KIND_BYTES: i32 = -1_879_048_192;
const KIND_BIGNUM: i32 = -2_147_483_648;

/// What a key parameter is tagged with.
const TAG_PURPOSE: i32 = 536_870_913;
const TAG_BLOCK_MODE: i32 = 536_870_916;
const TAG_PADDING: i32 = 536_870_918;
const TAG_MAC_LENGTH: i32 = 805_307_371;
const TAG_APPLICATION_ID: i32 = -1_879_047_591;
const TAG_NONCE: i32 = -1_879_047_191;

/// How big a key parameter is in memory: its tag, the union of its numbers,
/// and the frame of the run of bytes it may carry instead.
const PARAMETER_BYTES: usize = 32;

/// How big a hardware authentication token and a verification token are in
/// memory, and where in each a run of bytes sits.
const TOKEN_BYTES: usize = 56;
const TOKEN_MAC: u64 = 40;
const VERIFIED_BYTES: u64 = 16;

/// What the phone says about its key hardware.
pub struct Hardware {
    pub security_level: i32,
    pub name: String,
    pub author: String,
}

/// One key parameter, as the far side reads it: the tag, the number it
/// carries in the union, and the run of bytes it may carry instead.
struct Parameter {
    tag: i32,
    long: u64,
    blob: Vec<u8>,
}

impl Parameter {
    fn number(tag: i32, value: i32) -> Parameter {
        Parameter { tag, long: u64::from(value as u32), blob: Vec::new() }
    }

    fn bytes(tag: i32, blob: &[u8]) -> Parameter {
        Parameter { tag, long: 0, blob: blob.to_vec() }
    }
}

/// Write a run of key parameters as the far side reads one: the frame
/// saying how many, then the parameters, and for each the run of bytes it
/// carries.
fn parameters(call: &mut Message, params: &[Parameter]) {
    let mut elements = vec![0u8; params.len() * PARAMETER_BYTES];
    let mut inside = Vec::new();
    for (i, p) in params.iter().enumerate() {
        let at = i * PARAMETER_BYTES;
        elements[at..at + 4].copy_from_slice(&(p.tag as u32).to_le_bytes());
        // The union holds whichever number the tag says; a run of bytes is
        // carried beside it, in the frame that follows.
        elements[at + 8..at + 16].copy_from_slice(&p.long.to_le_bytes());
        elements[at + 24..at + 28].copy_from_slice(&(p.blob.len() as u32).to_le_bytes());
        inside.push((at as u64 + 16, p.blob.clone()));
    }
    let inside: Vec<(u64, &[u8])> = inside.iter().map(|(offset, bytes)| (*offset, bytes.as_slice())).collect();
    call.structures(params.len(), &elements, &inside);
}

/// The checks a key service makes against a storage key: it is decrypted
/// with AES-GCM, under the nonce the phone recorded, with the tag as long
/// as the phone wrote it, and bound to the application id the key was made
/// with.
fn gcm_params(app_id: &[u8], nonce: &[u8]) -> Vec<Parameter> {
    let mut params = vec![
        Parameter::number(TAG_BLOCK_MODE, BLOCK_MODE_GCM as i32),
        Parameter::number(TAG_PADDING, PADDING_NONE as i32),
        Parameter::number(TAG_MAC_LENGTH, 128),
    ];
    if !app_id.is_empty() {
        params.push(Parameter::bytes(TAG_APPLICATION_ID, app_id));
    }
    params.push(Parameter::bytes(TAG_NONCE, nonce));
    params.push(Parameter::number(TAG_PURPOSE, PURPOSE_DECRYPT as i32));
    params
}

/// The note the secure world wrote when the user answered their screen
/// lock, as the key service takes it, and the run of bytes inside it. A key
/// guarded by nothing takes a token of nothing, which is written the same
/// way.
fn auth_token(note: Option<&Token>) -> (Vec<u8>, Option<Vec<u8>>) {
    let mut bytes = vec![0u8; TOKEN_BYTES];
    let mac = note.map(|note| {
        bytes[0..8].copy_from_slice(&note.challenge.to_le_bytes());
        bytes[8..16].copy_from_slice(&note.user.to_le_bytes());
        bytes[16..24].copy_from_slice(&note.authenticator.to_le_bytes());
        bytes[24..28].copy_from_slice(&(note.kind as u32).to_le_bytes());
        bytes[32..40].copy_from_slice(&note.when.to_le_bytes());
        bytes[TOKEN_MAC as usize..TOKEN_MAC as usize + 4].copy_from_slice(&(note.mac.len() as u32).to_le_bytes());
        note.mac.clone()
    });
    (bytes, mac)
}

/// A verification token that verifies nothing, which is what a decryption
/// the secure world itself opened takes.
fn verification_token() -> Vec<u8> {
    vec![0u8; TOKEN_BYTES]
}

/// The phone's key service, as this program talks to it.
pub struct Keymaster<'a> {
    device: Handle<'a>,
}

impl<'a> Keymaster<'a> {
    /// Reach the running key service, or say that none is.
    pub fn open(hardware: &'a Binder) -> Result<Keymaster<'a>> {
        let not_running = || format!("{SERVICE}/{INSTANCE} is not running; the phone's key service has to be started first");
        let device = hidl::find(hardware, SERVICE, INSTANCE)?.with_context(not_running)?;
        Ok(Keymaster { device })
    }

    pub fn hardware(&self) -> Result<Hardware> {
        let reply = self.device.transact(GET_HARDWARE_INFO, &Message::call(INTERFACE)).context("ask the key service about itself")?;
        let mut read = Answer::new(reply.bytes());
        read.status().context("ask the key service about itself")?;
        let security_level = read.i32().context("read the security level of the key hardware")?;
        let name = text(&mut read).context("read the name of the key hardware")?;
        let author = text(&mut read).context("read the name of its author")?;
        Ok(Hardware { security_level, name, author })
    }

    /// The key a directory holds, in the form the storage hardware takes for
    /// this start of the phone. The secure world hands the key back wrapped
    /// for the hardware rather than as itself.
    pub fn ephemeral_storage_key(&self, key: &[u8]) -> Result<Vec<u8>> {
        match self.export(key) {
            Err(e) if refused_with(&e) == Some(KEY_REQUIRES_UPGRADE) => {
                let renewed = self.upgrade(key, &[]).context("renew the storage key for the firmware running now")?;
                self.export(&renewed)
            }
            exported => exported,
        }
    }

    fn export(&self, key: &[u8]) -> Result<Vec<u8>> {
        let mut call = Message::call(INTERFACE);
        call.u32(FORMAT_RAW);
        call.bytes(key);
        call.bytes(&[]);
        call.bytes(&[]);
        let reply = self.device.transact(EXPORT_KEY, &call).context("make a storage key for this start")?;
        let mut read = Answer::new(reply.bytes());
        read.status().context("make a storage key for this start")?;
        let error = read.i32().context("read whether the key service exported the key")?;
        if error != 0 {
            return Err(refusal(error, "the key service refused to export the key"));
        }
        bytes(&mut read).context("read the storage key")
    }

    /// What the key service says a key may be used for: each entry is a tag
    /// and the number or length that came with it.
    pub fn authorizations(&self, key_blob: &[u8], app_id: &[u8]) -> Result<Vec<(i32, i64)>> {
        let mut call = Message::call(INTERFACE);
        call.bytes(key_blob);
        call.bytes(app_id);
        call.bytes(&[]);
        let reply = self.device.transact(GET_KEY_CHARACTERISTICS, &call).context("ask what the key may be used for")?;
        let mut read = Answer::new(reply.bytes());
        read.status().context("ask what the key may be used for")?;
        let error = read.i32().context("read whether the key service answered")?;
        if error != 0 {
            return Err(refusal(error, "the key service refused to say what the key may be used for"));
        }
        let characteristics = read.pointed().context("read what the key may be used for")?;
        ensure!(characteristics.len() >= 32, "the key service answered {} bytes of characteristics", characteristics.len());
        let mut out = Vec::new();
        for at in [0usize, 16] {
            let count = u32::from_le_bytes(characteristics[at + 8..at + 12].try_into().expect("four bytes")) as usize;
            let at_ptr = u64::from_le_bytes(characteristics[at..at + 8].try_into().expect("eight bytes"));
            if count == 0 {
                continue;
            }
            ensure!(at_ptr != 0, "the key service named {count} parameters but held none");
            let elements = unsafe { std::slice::from_raw_parts(at_ptr as *const u8, count * PARAMETER_BYTES) };
            for element in elements.as_chunks::<PARAMETER_BYTES>().0 {
                out.push(parameter(element)?);
            }
        }
        Ok(out)
    }

    /// Undo an AES-GCM encryption the key service did with `key_blob`. The
    /// service only hands back the plaintext when `app_id` is the one the
    /// key was made for, so a wrong secret fails here rather than yielding
    /// something that looks like a key. A key guarded by a screen lock
    /// takes the note the secure world wrote when it was answered.
    pub fn gcm_decrypt(&self, key_blob: &[u8], app_id: &[u8], nonce: &[u8], body: &[u8], note: Option<&Token>) -> Result<Vec<u8>> {
        let operation = match self.begin(key_blob, app_id, nonce, note) {
            Ok(operation) => operation,
            Err(e) if refused_with(&e) == Some(KEY_REQUIRES_UPGRADE) => {
                let renewed = self
                    .upgrade(key_blob, &gcm_params(app_id, nonce))
                    .context("renew the key for the firmware running now")?;
                self.begin(&renewed, app_id, nonce, note)?
            }
            Err(e) => return Err(e),
        };
        let mut out = self.update(operation, body)?;
        out.extend_from_slice(&self.finish(operation)?);
        Ok(out)
    }

    fn begin(&self, key_blob: &[u8], app_id: &[u8], nonce: &[u8], note: Option<&Token>) -> Result<u64> {
        let mut call = Message::call(INTERFACE);
        call.u32(PURPOSE_DECRYPT);
        call.bytes(key_blob);
        parameters(&mut call, &gcm_params(app_id, nonce));
        let (token, mac) = auth_token(note);
        call.structure(&token, &[(TOKEN_MAC, mac.as_deref().unwrap_or(&[]))]);

        let reply = self.device.transact(BEGIN, &call).context("start a decryption")?;
        let mut read = Answer::new(reply.bytes());
        read.status().context("start a decryption")?;
        let error = read.i32().context("read whether the key service started the decryption")?;
        if error != 0 {
            return Err(refusal(error, "the key service refused to start the decryption"));
        }
        // What the decryption came back with, which a kernel-cleared key
        // has nothing to say about.
        let _out = params(&mut read).context("read what the decryption came back with")?;
        read.u64().context("read which decryption the key service opened")
    }

    /// A key for the firmware running now, from one made under an older
    /// build. What it is bound to has to be given again, or the service
    /// cannot tell it is the right key.
    fn upgrade(&self, key_blob: &[u8], params: &[Parameter]) -> Result<Vec<u8>> {
        let mut call = Message::call(INTERFACE);
        call.bytes(key_blob);
        parameters(&mut call, params);
        let reply = self.device.transact(UPGRADE_KEY, &call).context("renew the key")?;
        let mut read = Answer::new(reply.bytes());
        read.status().context("renew the key")?;
        let error = read.i32().context("read whether the key service renewed the key")?;
        if error != 0 {
            return Err(refusal(error, "the key service refused to renew the key"));
        }
        bytes(&mut read).context("read the renewed key")
    }

    fn update(&self, operation: u64, input: &[u8]) -> Result<Vec<u8>> {
        let mut call = Message::call(INTERFACE);
        call.u64(operation);
        parameters(&mut call, &[]);
        call.bytes(input);
        call.structure(&auth_token(None).0, &[(TOKEN_MAC, &[])]);
        call.structure(&verification_token(), &[(VERIFIED_BYTES, &[]), (TOKEN_MAC, &[])]);
        let reply = self.device.transact(UPDATE, &call).context("feed the decryption")?;
        let mut read = Answer::new(reply.bytes());
        read.status().context("feed the decryption")?;
        let error = read.i32().context("read whether the key service fed the decryption")?;
        if error != 0 {
            return Err(refusal(error, "the key service refused to go on with the decryption"));
        }
        read.i32().context("read how much of what was fed the decryption it took")?;
        let _out = params(&mut read).context("read what the decryption came back with")?;
        bytes(&mut read).context("read what the decryption gave back")
    }

    fn finish(&self, operation: u64) -> Result<Vec<u8>> {
        let mut call = Message::call(INTERFACE);
        call.u64(operation);
        parameters(&mut call, &[]);
        call.bytes(&[]);
        call.bytes(&[]);
        call.structure(&auth_token(None).0, &[(TOKEN_MAC, &[])]);
        call.structure(&verification_token(), &[(VERIFIED_BYTES, &[]), (TOKEN_MAC, &[])]);
        let reply = self.device.transact(FINISH, &call).context("close the decryption")?;
        let mut read = Answer::new(reply.bytes());
        read.status().context("close the decryption")?;
        let error = read.i32().context("read whether the key service closed the decryption")?;
        if error != 0 {
            return Err(refusal(error, "the key service closed the decryption with a refusal, which is where a wrong key is turned down"));
        }
        let _out = params(&mut read).context("read what the decryption came back with")?;
        bytes(&mut read).context("read what the decryption ended with")
    }

    /// Whether the secure world will unwrap the key at all: a decryption is
    /// opened on it under a nonce of nothing and dropped again, which reads
    /// nothing and changes nothing.
    pub fn usable(&self, key_blob: &[u8], app_id: &[u8]) -> Result<()> {
        let operation = self.begin(key_blob, app_id, &[0; NONCE_BYTES], None)?;
        let mut call = Message::call(INTERFACE);
        call.u64(operation);
        let reply = self.device.transact(ABORT, &call).context("drop the decryption")?;
        let mut read = Answer::new(reply.bytes());
        read.status().context("drop the decryption")?;
        let error = read.i32().context("read whether the key service dropped the decryption")?;
        if error != 0 {
            return Err(refusal(error, "the key service refused to drop the decryption"));
        }
        Ok(())
    }
}

/// The refusal the key service wrote, where it was one it named with the
/// code this program acts on.
fn refused_with(e: &anyhow::Error) -> Option<i32> {
    e.chain().find_map(|cause| cause.downcast_ref::<Refusal>().map(|r| r.0))
}

/// A refusal the key service wrote, kept as its code so that the one this
/// program acts on can be told apart from the rest.
#[derive(Debug)]
struct Refusal(i32);

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the key service answered {}", self.0)
    }
}

impl std::error::Error for Refusal {}

/// What the key service refused with, kept as the refusal's own code so
/// that the one this program acts on can be told apart from the rest.
fn refusal(error: i32, what: &str) -> anyhow::Error {
    anyhow::Error::new(Refusal(error)).context(what.to_string())
}

/// A run of bytes the far side wrote as a frame and then the bytes
/// themselves.
fn bytes(read: &mut Answer) -> Result<Vec<u8>> {
    let frame = read.pointed().context("read the frame of a run of bytes")?;
    ensure!(frame.len() >= 16, "a run of bytes is framed in {} bytes", frame.len());
    let count = u32::from_le_bytes(frame[8..12].try_into().expect("four bytes")) as usize;
    match read.opt_pointed().context("read a run of bytes")? {
        Some(bytes) => {
            ensure!(bytes.len() == count, "a run of {count} bytes carries {}", bytes.len());
            Ok(bytes.to_vec())
        }
        None => {
            ensure!(count == 0, "a run of {count} bytes points at nothing");
            Ok(Vec::new())
        }
    }
}

/// Text the far side wrote the same way, with a terminator the count leaves
/// out.
fn text(read: &mut Answer) -> Result<String> {
    let frame = read.pointed().context("read the frame of a text")?;
    ensure!(frame.len() >= 16, "a text is framed in {} bytes", frame.len());
    let count = u32::from_le_bytes(frame[8..12].try_into().expect("four bytes")) as usize;
    let bytes = read.pointed().context("read a text")?;
    ensure!(bytes.len() == count + 1, "a text of {count} characters carries {} bytes", bytes.len());
    Ok(String::from_utf8_lossy(&bytes[..count]).into_owned())
}

/// The key parameters the far side wrote, as their tag and what came with
/// it.
fn params(read: &mut Answer) -> Result<Vec<(i32, i64)>> {
    let frame = read.pointed().context("read the frame of key parameters")?;
    ensure!(frame.len() >= 16, "key parameters are framed in {} bytes", frame.len());
    let count = u32::from_le_bytes(frame[8..12].try_into().expect("four bytes")) as usize;
    let Some(elements) = read.opt_pointed().context("read key parameters")? else {
        ensure!(count == 0, "{count} key parameters point at nothing");
        return Ok(Vec::new());
    };
    ensure!(elements.len() >= count * PARAMETER_BYTES, "{} key parameters are written in {} bytes", count, elements.len());
    let mut out = Vec::new();
    for element in elements[..count * PARAMETER_BYTES].as_chunks::<PARAMETER_BYTES>().0 {
        out.push(parameter(element)?);
    }
    Ok(out)
}

/// One key parameter as the far side wrote it: the tag, the number that
/// came with it, and the run of bytes it carries.
fn parameter(element: &[u8; PARAMETER_BYTES]) -> Result<(i32, i64)> {
    let tag = i32::from_le_bytes(element[0..4].try_into().expect("four bytes"));
    let value = i64::from_le_bytes(element[8..16].try_into().expect("eight bytes"));
    let length = u32::from_le_bytes(element[24..28].try_into().expect("four bytes")) as i64;
    let value = match tag & TAG_KIND {
        KIND_ULONG | KIND_ULONG_REP | KIND_DATE => value,
        KIND_BYTES | KIND_BIGNUM => length,
        _ => value & 0xffff_ffff,
    };
    Ok((tag, value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use skyward_binder::parcel::Payload;

    #[test]
    fn a_parameter_carries_its_tag_and_number_in_the_room_the_far_side_reads() {
        let mut call = Message::call(INTERFACE);
        parameters(&mut call, &[Parameter::number(TAG_BLOCK_MODE, BLOCK_MODE_GCM as i32), Parameter::bytes(TAG_NONCE, &[1; 12])]);
        let data = call.values().to_vec();
        // The call opens with its interface name, then the frame of the run.
        let at = data.iter().position(|b| *b == 0).expect("a terminator") + 1;
        let frame = &data[at..];
        assert_eq!(u32::from_le_bytes(frame[8..12].try_into().unwrap()), 2, "two parameters");
    }

    #[test]
    fn a_note_is_written_where_the_far_side_reads_its_fields() {
        let note = Token { challenge: 7, user: 8, authenticator: 9, kind: 1, when: 10, mac: vec![3; 32] };
        let (bytes, mac) = auth_token(Some(&note));
        assert_eq!(bytes.len(), TOKEN_BYTES);
        assert_eq!(u64::from_le_bytes(bytes[0..8].try_into().unwrap()), 7);
        assert_eq!(u64::from_le_bytes(bytes[8..16].try_into().unwrap()), 8);
        assert_eq!(u64::from_le_bytes(bytes[16..24].try_into().unwrap()), 9);
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 1);
        assert_eq!(u64::from_le_bytes(bytes[32..40].try_into().unwrap()), 10);
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 32);
        assert_eq!(mac.as_deref(), Some(&[3u8; 32][..]));
    }
}
