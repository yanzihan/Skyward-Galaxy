pub mod encryption;
pub mod fscrypt;
pub mod gatekeeper;
pub mod keystore;
pub mod locksettings;
pub mod sharedsecret;
pub mod token;
pub mod user;
pub mod weaver;

#[cfg(target_os = "android")]
pub mod data;
#[cfg(target_os = "android")]
pub mod keymaster;
#[cfg(target_os = "android")]
pub mod keymint;
#[cfg(target_os = "android")]
pub mod keystore2;

/// The phone's key service, of whichever kind the phone keeps: the newer
/// kind, registered with the service directory, or the older kind, reached
/// over the driver the phone's hardware services talk on. What the recovery
/// asks of a key service is the same either way.
#[cfg(target_os = "android")]
pub enum Service<'a> {
    KeyMint(keymint::KeyMint<'a>),
    Keymaster(keymaster::Keymaster<'a>),
}

#[cfg(target_os = "android")]
impl<'a> Service<'a> {
    /// Reach the running key service, of the newer kind where the phone has
    /// one and of the older kind otherwise, or say that none is. A vendor
    /// build for Android 12 or later keeps its newer kind with the vendor
    /// partition's own service directory, so that driver is asked too, where
    /// the phone has it: one without it is reached as it always was.
    pub fn open(
        binder: &'a skyward_binder::driver::Binder,
        vendor: Option<&'a skyward_binder::driver::Binder>,
        hardware: &'a skyward_binder::driver::Binder,
    ) -> anyhow::Result<Service<'a>> {
        for at in [Some(binder), vendor].into_iter().flatten() {
            if let Ok(service) = keymint::KeyMint::open(at) {
                return Ok(Service::KeyMint(service));
            }
        }
        Ok(Service::Keymaster(keymaster::Keymaster::open(hardware)?))
    }

    /// Turn a key that only the storage hardware can use into the form it
    /// takes for this start of the phone.
    pub fn ephemeral_storage_key(&self, key: &[u8]) -> anyhow::Result<Vec<u8>> {
        match self {
            Service::KeyMint(service) => service.ephemeral_storage_key(key),
            Service::Keymaster(service) => service.ephemeral_storage_key(key),
        }
    }

    /// Undo an AES-GCM encryption the key service did with `key_blob`. A
    /// key guarded by the screen lock takes the note the secure world wrote
    /// when it was answered.
    pub fn gcm_decrypt(
        &self,
        key_blob: &[u8],
        app_id: &[u8],
        nonce: &[u8],
        body: &[u8],
        note: Option<&token::Token>,
    ) -> anyhow::Result<Vec<u8>> {
        match self {
            Service::KeyMint(service) => service.gcm_decrypt(key_blob, app_id, nonce, body, note),
            Service::Keymaster(service) => service.gcm_decrypt(key_blob, app_id, nonce, body, note),
        }
    }

    /// What the key service says a key may be used for: each entry is a tag
    /// and the number or length that came with it.
    pub fn authorizations(&self, key_blob: &[u8], app_id: &[u8]) -> anyhow::Result<Vec<(i32, i64)>> {
        match self {
            Service::KeyMint(service) => service.authorizations(key_blob, app_id),
            Service::Keymaster(service) => service.authorizations(key_blob, app_id),
        }
    }

    /// Whether the secure world will unwrap the key at all.
    pub fn usable(&self, key_blob: &[u8], app_id: &[u8]) -> anyhow::Result<()> {
        match self {
            Service::KeyMint(service) => service.usable(key_blob, app_id),
            Service::Keymaster(service) => service.usable(key_blob, app_id),
        }
    }
}

#[cfg(target_os = "android")]
impl keystore::Decryptor for Service<'_> {
    fn gcm_decrypt(&self, key_blob: &[u8], app_id: &[u8], nonce: &[u8], body: &[u8]) -> anyhow::Result<Vec<u8>> {
        // The keys unwrapped this way belong to the phone rather than to
        // any user, so none of them is guarded by a screen lock.
        Service::gcm_decrypt(self, key_blob, app_id, nonce, body, None)
    }
}
