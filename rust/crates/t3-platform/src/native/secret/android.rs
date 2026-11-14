//! The storage key wrapped by an AES-GCM key that never leaves the Android Keystore.
//!
//! Only the wrapped form is written, beside the app's data; unwrapping needs this app's Keystore
//! entry, which the OS drops with the app. If that entry is gone, the wrapped key reads as
//! missing, so the store starts with a fresh key and discards secure records it can no longer
//! open, as it does on desktop.
use super::{SecretKeyStore, SecretStoreError, wrapped};
use jni::{
    JNIEnv, JavaVM,
    objects::{JByteArray, JObject, JValue},
};
use std::{io::ErrorKind, path::PathBuf, sync::OnceLock};
use zeroize::Zeroizing;

static VM: OnceLock<JavaVM> = OnceLock::new();

/// Gives the platform the app's Java VM, once, before [`AndroidKeystoreStore::new`]; the
/// Android host holds it from `JNI_OnLoad`. Returns `false` if one was already set.
pub fn set_android_vm(vm: JavaVM) -> bool {
    VM.set(vm).is_ok()
}

const ALIAS: &str = "t3code.client-storage-key";
const ENCRYPT_MODE: i32 = 1;
const DECRYPT_MODE: i32 = 2;
const PURPOSE_ENCRYPT_DECRYPT: i32 = 1 | 2;
const TAG_BITS: i32 = 128;
const BUILDER: &str = "android/security/keystore/KeyGenParameterSpec$Builder";
const BUILDER_RETURN: &str = "Landroid/security/keystore/KeyGenParameterSpec$Builder;";

pub struct AndroidKeystoreStore {
    vm: &'static JavaVM,
    dir: PathBuf,
}

impl AndroidKeystoreStore {
    /// Keeps wrapped keys in `dir`, which must be app-private.
    /// The Java VM is the one given to [`set_android_vm`], or else the process's VM found
    /// through the NDK (API 31+).
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self, SecretStoreError> {
        if VM.get().is_none()
            && let Some(vm) = created_vm()
        {
            let _ = VM.set(vm);
        }
        let vm = VM.get().ok_or_else(|| {
            SecretStoreError::Unavailable(
                "no Java VM: none was given to set_android_vm and JNI_GetCreatedJavaVMs is unavailable before API 31".into(),
            )
        })?;
        Ok(Self {
            vm,
            dir: dir.into(),
        })
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(wrapped::file_name(id))
    }

    fn java<R>(
        &self,
        call: impl for<'local> FnOnce(&mut JNIEnv<'local>) -> jni::errors::Result<R>,
    ) -> Result<R, JavaFailure> {
        let mut env = self
            .vm
            .attach_current_thread()
            .map_err(|error| JavaFailure::Other(error.to_string()))?;
        // A frame frees the call's local references on a thread Java already attached.
        let result = env.with_local_frame(32, |env| call(env));
        match result {
            Ok(value) => Ok(value),
            Err(jni::errors::Error::JavaException) => Err(classify_exception(&mut env)),
            Err(error) => Err(JavaFailure::Other(error.to_string())),
        }
    }
}

/// How a Keystore call failed.
enum JavaFailure {
    /// The key cannot open this blob or is gone for good (a tag mismatch, backup data
    /// restored onto a device with its own key, an invalidated key): treat it as missing.
    KeyUnusable,
    /// Anything else, including a transient Keystore or provider error: fail this launch only.
    Other(String),
}

/// Exceptions that mean the stored key will never open this blob again.
const KEY_UNUSABLE: &[&str] = &[
    "javax/crypto/AEADBadTagException",
    "javax/crypto/BadPaddingException",
    "java/security/UnrecoverableKeyException",
    "android/security/keystore/KeyPermanentlyInvalidatedException",
];

fn classify_exception(env: &mut JNIEnv<'_>) -> JavaFailure {
    let Ok(throwable) = env.exception_occurred() else {
        return JavaFailure::Other("an unreadable Java exception".into());
    };
    let _ = env.exception_describe();
    let _ = env.exception_clear();
    for class in KEY_UNUSABLE {
        // A class the device lacks raises NoClassDefFoundError; clear it and move on.
        let matched = env.is_instance_of(&throwable, *class).unwrap_or(false);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        if matched {
            return JavaFailure::KeyUnusable;
        }
    }
    JavaFailure::Other("a Keystore call failed".into())
}

fn failure_to_secret_error(failure: JavaFailure) -> SecretStoreError {
    match failure {
        JavaFailure::KeyUnusable => SecretStoreError::Corrupt,
        JavaFailure::Other(detail) => SecretStoreError::Unavailable(detail),
    }
}

fn delete_keystore_key(env: &mut JNIEnv<'_>) -> jni::errors::Result<()> {
    let provider = env.new_string("AndroidKeyStore")?;
    let store = env
        .call_static_method(
            "java/security/KeyStore",
            "getInstance",
            "(Ljava/lang/String;)Ljava/security/KeyStore;",
            &[JValue::Object(&provider)],
        )?
        .l()?;
    env.call_method(
        &store,
        "load",
        "(Ljava/security/KeyStore$LoadStoreParameter;)V",
        &[JValue::Object(&JObject::null())],
    )?;
    let alias = env.new_string(ALIAS)?;
    env.call_method(
        &store,
        "deleteEntry",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&alias)],
    )?;
    Ok(())
}

/// Writes `bytes` to `path` so that a crash leaves either the old file or the new one whole.
fn write_durably(directory: &std::path::Path, path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let partial = path.with_extension("partial");
    let mut file = std::fs::File::create(&partial)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&partial, path)?;
    std::fs::File::open(directory)?.sync_all()
}

/// The process's Java VM through the NDK's public `JNI_GetCreatedJavaVMs`, which
/// libnativehelper exports from API 31, for hosts that do not call `set_android_vm`.
#[allow(unsafe_code)]
fn created_vm() -> Option<JavaVM> {
    use std::ffi::{c_char, c_int, c_void};
    #[link(name = "dl")]
    unsafe extern "C" {
        fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }
    const RTLD_NOW: c_int = 2;
    type GetCreatedJavaVms = unsafe extern "system" fn(
        *mut *mut jni::sys::JavaVM,
        jni::sys::jsize,
        *mut jni::sys::jsize,
    ) -> jni::sys::jint;
    // SAFETY: both names are NUL-terminated; the library handle stays open for the process.
    let handle = unsafe { dlopen(c"libnativehelper.so".as_ptr(), RTLD_NOW) };
    if handle.is_null() {
        return None;
    }
    // SAFETY: as above.
    let symbol = unsafe { dlsym(handle, c"JNI_GetCreatedJavaVMs".as_ptr()) };
    if symbol.is_null() {
        return None;
    }
    // SAFETY: `jni.h` declares `JNI_GetCreatedJavaVMs` with exactly this signature.
    let get_created = unsafe { std::mem::transmute::<*mut c_void, GetCreatedJavaVms>(symbol) };
    let mut vm: *mut jni::sys::JavaVM = std::ptr::null_mut();
    let mut count: jni::sys::jsize = 0;
    // SAFETY: one output slot; Android has a single VM per process.
    let status = unsafe { get_created(&mut vm, 1, &mut count) };
    if status != jni::sys::JNI_OK || count < 1 || vm.is_null() {
        return None;
    }
    // SAFETY: the process's VM, valid for the life of the process.
    unsafe { JavaVM::from_raw(vm) }.ok()
}

fn unavailable(error: impl std::fmt::Display) -> SecretStoreError {
    SecretStoreError::Unavailable(error.to_string())
}

/// This app's Keystore key; generated when `create` and absent.
fn keystore_key<'local>(
    env: &mut JNIEnv<'local>,
    create: bool,
) -> jni::errors::Result<Option<JObject<'local>>> {
    let provider = env.new_string("AndroidKeyStore")?;
    let store = env
        .call_static_method(
            "java/security/KeyStore",
            "getInstance",
            "(Ljava/lang/String;)Ljava/security/KeyStore;",
            &[JValue::Object(&provider)],
        )?
        .l()?;
    env.call_method(
        &store,
        "load",
        "(Ljava/security/KeyStore$LoadStoreParameter;)V",
        &[JValue::Object(&JObject::null())],
    )?;
    let alias = env.new_string(ALIAS)?;
    let key = env
        .call_method(
            &store,
            "getKey",
            "(Ljava/lang/String;[C)Ljava/security/Key;",
            &[JValue::Object(&alias), JValue::Object(&JObject::null())],
        )?
        .l()?;
    if !key.is_null() {
        return Ok(Some(key));
    }
    if !create {
        return Ok(None);
    }
    let builder = env.new_object(
        BUILDER,
        "(Ljava/lang/String;I)V",
        &[JValue::Object(&alias), JValue::Int(PURPOSE_ENCRYPT_DECRYPT)],
    )?;
    let gcm = env.new_string("GCM")?;
    let modes = env.new_object_array(1, "java/lang/String", &gcm)?;
    env.call_method(
        &builder,
        "setBlockModes",
        format!("([Ljava/lang/String;){BUILDER_RETURN}"),
        &[JValue::Object(&modes)],
    )?;
    let no_padding = env.new_string("NoPadding")?;
    let paddings = env.new_object_array(1, "java/lang/String", &no_padding)?;
    env.call_method(
        &builder,
        "setEncryptionPaddings",
        format!("([Ljava/lang/String;){BUILDER_RETURN}"),
        &[JValue::Object(&paddings)],
    )?;
    env.call_method(
        &builder,
        "setKeySize",
        format!("(I){BUILDER_RETURN}"),
        &[JValue::Int(256)],
    )?;
    let spec = env
        .call_method(
            &builder,
            "build",
            "()Landroid/security/keystore/KeyGenParameterSpec;",
            &[],
        )?
        .l()?;
    let aes = env.new_string("AES")?;
    let generator = env
        .call_static_method(
            "javax/crypto/KeyGenerator",
            "getInstance",
            "(Ljava/lang/String;Ljava/lang/String;)Ljavax/crypto/KeyGenerator;",
            &[JValue::Object(&aes), JValue::Object(&provider)],
        )?
        .l()?;
    env.call_method(
        &generator,
        "init",
        "(Ljava/security/spec/AlgorithmParameterSpec;)V",
        &[JValue::Object(&spec)],
    )?;
    Ok(Some(
        env.call_method(&generator, "generateKey", "()Ljavax/crypto/SecretKey;", &[])?
            .l()?,
    ))
}

fn cipher<'local>(env: &mut JNIEnv<'local>) -> jni::errors::Result<JObject<'local>> {
    let name = env.new_string("AES/GCM/NoPadding")?;
    env.call_static_method(
        "javax/crypto/Cipher",
        "getInstance",
        "(Ljava/lang/String;)Ljavax/crypto/Cipher;",
        &[JValue::Object(&name)],
    )?
    .l()
}

fn do_final(
    env: &mut JNIEnv<'_>,
    cipher: &JObject<'_>,
    input: &[u8],
) -> jni::errors::Result<Vec<u8>> {
    let input = env.byte_array_from_slice(input)?;
    let output = JByteArray::from(
        env.call_method(cipher, "doFinal", "([B)[B", &[JValue::Object(&input)])?
            .l()?,
    );
    env.convert_byte_array(&output)
}

impl SecretKeyStore for AndroidKeystoreStore {
    fn load(&self, id: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretStoreError> {
        let blob = match std::fs::read(self.path(id)) {
            Ok(blob) => blob,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(unavailable(error)),
        };
        let (nonce, ciphertext) = wrapped::decode(&blob)?;
        self.java(|env| {
            let Some(key) = keystore_key(env, false)? else {
                return Ok(None);
            };
            let cipher = cipher(env)?;
            let iv = env.byte_array_from_slice(nonce)?;
            let spec = env.new_object(
                "javax/crypto/spec/GCMParameterSpec",
                "(I[B)V",
                &[JValue::Int(TAG_BITS), JValue::Object(&iv)],
            )?;
            env.call_method(
                &cipher,
                "init",
                "(ILjava/security/Key;Ljava/security/spec/AlgorithmParameterSpec;)V",
                &[
                    JValue::Int(DECRYPT_MODE),
                    JValue::Object(&key),
                    JValue::Object(&spec),
                ],
            )?;
            do_final(env, &cipher, ciphertext).map(|plain| Some(Zeroizing::new(plain)))
        })
        .map_err(failure_to_secret_error)
    }

    fn store(&self, id: &str, secret: &[u8]) -> Result<(), SecretStoreError> {
        let wrap = || {
            self.java(|env| {
                let key =
                    keystore_key(env, true)?.ok_or(jni::errors::Error::NullPtr("keystore key"))?;
                let cipher = cipher(env)?;
                env.call_method(
                    &cipher,
                    "init",
                    "(ILjava/security/Key;)V",
                    &[JValue::Int(ENCRYPT_MODE), JValue::Object(&key)],
                )?;
                let ciphertext = do_final(env, &cipher, secret)?;
                let iv = JByteArray::from(env.call_method(&cipher, "getIV", "()[B", &[])?.l()?);
                Ok((env.convert_byte_array(&iv)?, ciphertext))
            })
        };
        // A key invalidated for good cannot wrap a new secret either: replace it once.
        let (nonce, ciphertext) = match wrap() {
            Err(JavaFailure::KeyUnusable) => {
                self.java(delete_keystore_key).map_err(failure_to_secret_error)?;
                wrap()
            }
            result => result,
        }
        .map_err(failure_to_secret_error)?;
        std::fs::create_dir_all(&self.dir).map_err(unavailable)?;
        write_durably(
            &self.dir,
            &self.path(id),
            &wrapped::encode(&nonce, &ciphertext)?,
        )
        .map_err(unavailable)
    }
}
