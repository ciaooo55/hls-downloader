use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const HOST_NAME: &str = "com.ciaooo55.hls_downloader";
const HOST_DESCRIPTION: &str = "HLS Downloader Native Messaging Host";
const CHROMIUM_MANIFEST: &str = "HLSDownloaderNativeHost.chrome.json";
const FIREFOX_MANIFEST: &str = "HLSDownloaderNativeHost.firefox.json";
const NATIVE_HOST_EXE: &str = "HLSDownloaderNativeHost.exe";
const CHROMIUM_EXTENSION_ORIGIN: &str = "chrome-extension://bbdfldcjnikaemnimalegbopgaknjhla/";
const FIREFOX_EXTENSION_ID: &str = "hls-downloader-store@ciaooo55.com";

const CHROMIUM_REGISTRY_PARENTS: [&str; 6] = [
    r"Software\Google\Chrome\NativeMessagingHosts",
    r"Software\Microsoft\Edge\NativeMessagingHosts",
    r"Software\BraveSoftware\Brave-Browser\NativeMessagingHosts",
    r"Software\Chromium\NativeMessagingHosts",
    r"Software\Vivaldi\NativeMessagingHosts",
    r"Software\Opera Software\NativeMessagingHosts",
];
const FIREFOX_REGISTRY_PARENT: &str = r"Software\Mozilla\NativeMessagingHosts";

#[derive(Clone, Debug, Eq, PartialEq)]
struct ManifestPaths {
    chromium: PathBuf,
    firefox: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RegistrationEntry {
    key: String,
    manifest: PathBuf,
    allowlist_field: &'static str,
}

fn expected_host(engine: &Path, require_file: bool) -> Result<PathBuf, String> {
    if !engine.is_absolute() {
        return Err("Native Host registration requires an absolute Engine path".into());
    }
    let resources = engine
        .parent()
        .ok_or_else(|| "Engine path has no resource directory".to_string())?;
    let host = resources.join(NATIVE_HOST_EXE);
    if require_file && !host.is_file() {
        return Err(format!(
            "Native Host executable is missing: {}",
            host.display()
        ));
    }
    Ok(without_verbatim_prefix(&host))
}

fn without_verbatim_prefix(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    text.strip_prefix(r"\\?\")
        .map(PathBuf::from)
        .unwrap_or_else(|| path.to_path_buf())
}

fn fallback_manifest_directory() -> Result<PathBuf, String> {
    let local_app_data = std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "LOCALAPPDATA is unavailable for Native Host registration".to_string())?;
    Ok(PathBuf::from(local_app_data)
        .join("HLSDownloader")
        .join("v7-native-host"))
}

fn manifest_paths(directory: &Path) -> ManifestPaths {
    ManifestPaths {
        chromium: directory.join(CHROMIUM_MANIFEST),
        firefox: directory.join(FIREFOX_MANIFEST),
    }
}

fn manifest_bytes(host: &Path, allowlist_field: &str) -> Result<Vec<u8>, String> {
    if !host.is_absolute() {
        return Err("Native Host manifest path must be absolute".into());
    }
    let host_path = host.to_string_lossy().into_owned();
    let value = match allowlist_field {
        "allowed_origins" => json!({
            "name": HOST_NAME,
            "description": HOST_DESCRIPTION,
            "path": host_path,
            "type": "stdio",
            "allowed_origins": [CHROMIUM_EXTENSION_ORIGIN]
        }),
        "allowed_extensions" => json!({
            "name": HOST_NAME,
            "description": HOST_DESCRIPTION,
            "path": host_path,
            "type": "stdio",
            "allowed_extensions": [FIREFOX_EXTENSION_ID]
        }),
        _ => {
            return Err(format!(
                "unsupported Native Host allowlist: {allowlist_field}"
            ))
        }
    };
    let mut bytes = serde_json::to_vec_pretty(&value)
        .map_err(|error| format!("serialize Native Host manifest: {error}"))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn write_manifest_pair(directory: &Path, host: &Path) -> Result<ManifestPaths, String> {
    std::fs::create_dir_all(directory).map_err(|error| {
        format!(
            "create Native Host manifest directory {}: {error}",
            directory.display()
        )
    })?;
    let paths = manifest_paths(directory);
    let result = (|| {
        std::fs::write(&paths.chromium, manifest_bytes(host, "allowed_origins")?).map_err(
            |error| {
                format!(
                    "write Native Host manifest {}: {error}",
                    paths.chromium.display()
                )
            },
        )?;
        std::fs::write(&paths.firefox, manifest_bytes(host, "allowed_extensions")?).map_err(
            |error| {
                format!(
                    "write Native Host manifest {}: {error}",
                    paths.firefox.display()
                )
            },
        )?;
        Ok(paths.clone())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&paths.chromium);
        let _ = std::fs::remove_file(&paths.firefox);
    }
    result
}

fn prepare_manifests_with_fallback(
    engine: &Path,
    fallback: &Path,
) -> Result<(PathBuf, ManifestPaths), String> {
    let host = expected_host(engine, true)?;
    let preferred = engine
        .parent()
        .ok_or_else(|| "Engine path has no resource directory".to_string())?;
    match write_manifest_pair(preferred, &host) {
        Ok(paths) => Ok((host, paths)),
        Err(preferred_error) => write_manifest_pair(fallback, &host)
            .map(|paths| (host, paths))
            .map_err(|fallback_error| {
                format!(
                    "Native Host manifest generation failed in both locations; preferred: {preferred_error}; fallback: {fallback_error}"
                )
            }),
    }
}

fn prepare_manifests(engine: &Path) -> Result<(PathBuf, ManifestPaths), String> {
    prepare_manifests_with_fallback(engine, &fallback_manifest_directory()?)
}

fn registration_entries(paths: &ManifestPaths) -> Vec<RegistrationEntry> {
    let mut entries = CHROMIUM_REGISTRY_PARENTS
        .iter()
        .map(|parent| RegistrationEntry {
            key: format!(r"{parent}\{HOST_NAME}"),
            manifest: paths.chromium.clone(),
            allowlist_field: "allowed_origins",
        })
        .collect::<Vec<_>>();
    entries.push(RegistrationEntry {
        key: format!(r"{FIREFOX_REGISTRY_PARENT}\{HOST_NAME}"),
        manifest: paths.firefox.clone(),
        allowlist_field: "allowed_extensions",
    });
    entries
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn registry_error(action: &str, code: u32) -> String {
    format!(
        "{action}: {}",
        std::io::Error::from_raw_os_error(code as i32)
    )
}

#[cfg(windows)]
fn set_default_value(key: &str, value: &Path) -> Result<(), String> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY_CURRENT_USER, KEY_SET_VALUE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    let key_wide = wide(key);
    let value_wide = wide(&value.to_string_lossy());
    let mut handle = null_mut();
    let create = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            key_wide.as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            null(),
            &mut handle,
            null_mut(),
        )
    };
    if create != 0 {
        return Err(registry_error(&format!("create HKCU\\{key}"), create));
    }
    let bytes = unsafe {
        std::slice::from_raw_parts(
            value_wide.as_ptr().cast::<u8>(),
            value_wide.len() * std::mem::size_of::<u16>(),
        )
    };
    let set = unsafe {
        RegSetValueExW(
            handle,
            std::ptr::null(),
            0,
            REG_SZ,
            bytes.as_ptr(),
            bytes.len() as u32,
        )
    };
    unsafe {
        RegCloseKey(handle);
    }
    if set != 0 {
        return Err(registry_error(&format!("set HKCU\\{key}"), set));
    }
    Ok(())
}

#[cfg(windows)]
fn default_value(key: &str) -> Result<Option<PathBuf>, String> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};

    let key_wide = wide(key);
    let mut bytes = 0u32;
    let size_result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key_wide.as_ptr(),
            null(),
            RRF_RT_REG_SZ,
            null_mut(),
            null_mut(),
            &mut bytes,
        )
    };
    if size_result == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if size_result != 0 {
        return Err(registry_error(&format!("read HKCU\\{key}"), size_result));
    }
    let mut buffer = vec![0u16; (bytes as usize).div_ceil(std::mem::size_of::<u16>())];
    let read_result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key_wide.as_ptr(),
            null(),
            RRF_RT_REG_SZ,
            null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if read_result != 0 {
        return Err(registry_error(&format!("read HKCU\\{key}"), read_result));
    }
    while buffer.last() == Some(&0) {
        buffer.pop();
    }
    Ok(Some(PathBuf::from(String::from_utf16_lossy(&buffer))))
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

fn same_path(left: &Path, right: &Path) -> bool {
    normalized_path(left) == normalized_path(right)
}

fn manifest_is_owned(path: &Path, host: &Path, allowlist_field: &str) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    value.get("name").and_then(Value::as_str) == Some(HOST_NAME)
        && value.get("type").and_then(Value::as_str) == Some("stdio")
        && value
            .get("path")
            .and_then(Value::as_str)
            .map(Path::new)
            .is_some_and(|manifest_host| {
                manifest_host.is_absolute() && same_path(manifest_host, host)
            })
        && value
            .get(allowlist_field)
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
}

#[cfg(windows)]
fn delete_owned_key(
    key: &str,
    host: &Path,
    allowlist_field: &str,
) -> Result<Option<PathBuf>, String> {
    use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows_sys::Win32::System::Registry::{RegDeleteTreeW, HKEY_CURRENT_USER};

    let Some(current) = default_value(key)? else {
        return Ok(None);
    };
    if !manifest_is_owned(&current, host, allowlist_field) {
        return Ok(None);
    }
    let key_wide = wide(key);
    let result = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, key_wide.as_ptr()) };
    if result != 0 && result != ERROR_FILE_NOT_FOUND {
        return Err(registry_error(&format!("delete HKCU\\{key}"), result));
    }
    Ok(Some(current))
}

fn cleanup_owned_manifests(
    candidates: Vec<(PathBuf, &'static str)>,
    host: &Path,
) -> Result<usize, String> {
    let mut seen = HashSet::new();
    let mut removed = 0;
    for (path, allowlist_field) in candidates {
        if !seen.insert(normalized_path(&path)) || !manifest_is_owned(&path, host, allowlist_field)
        {
            continue;
        }
        std::fs::remove_file(&path).map_err(|error| {
            format!(
                "remove generated Native Host manifest {}: {error}",
                path.display()
            )
        })?;
        removed += 1;
    }
    Ok(removed)
}

pub fn register_packaged_native_host(engine: &Path) -> Result<usize, String> {
    let (_, manifests) = prepare_manifests(engine)?;
    let entries = registration_entries(&manifests);
    #[cfg(windows)]
    {
        for entry in &entries {
            set_default_value(&entry.key, &entry.manifest)?;
        }
        Ok(entries.len())
    }
    #[cfg(not(windows))]
    {
        let _ = entries;
        Ok(0)
    }
}

pub fn unregister_packaged_native_host(engine: &Path) -> Result<usize, String> {
    let host = expected_host(engine, false)?;
    let preferred = manifest_paths(
        engine
            .parent()
            .ok_or_else(|| "Engine path has no resource directory".to_string())?,
    );
    let fallback_directory = fallback_manifest_directory()?;
    let fallback = manifest_paths(&fallback_directory);
    let entries = registration_entries(&preferred);
    let mut candidates = vec![
        (preferred.chromium, "allowed_origins"),
        (preferred.firefox, "allowed_extensions"),
        (fallback.chromium, "allowed_origins"),
        (fallback.firefox, "allowed_extensions"),
    ];

    #[cfg(windows)]
    let removed_keys = {
        let mut removed = 0;
        for entry in &entries {
            if let Some(current) = delete_owned_key(&entry.key, &host, entry.allowlist_field)? {
                candidates.push((current, entry.allowlist_field));
                removed += 1;
            }
        }
        removed
    };
    #[cfg(not(windows))]
    let removed_keys = {
        let _ = entries;
        0
    };

    cleanup_owned_manifests(candidates, &host)?;
    if fallback_directory.is_dir()
        && std::fs::read_dir(&fallback_directory)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false)
    {
        let _ = std::fs::remove_dir(&fallback_directory);
    }
    Ok(removed_keys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture() -> (PathBuf, PathBuf) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("hls-v7-native-host-{nonce}"));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(NATIVE_HOST_EXE), b"fixture").unwrap();
        let engine = root.join("HLSDownloaderEngine.exe");
        fs::write(&engine, b"engine").unwrap();
        (engine, root)
    }

    #[test]
    fn generated_manifests_use_an_absolute_host_and_cover_all_browsers() {
        let (engine, root) = fixture();
        let (host, manifests) =
            prepare_manifests_with_fallback(&engine, &root.join("fallback")).unwrap();
        assert!(host.is_absolute());
        assert!(manifest_is_owned(
            &manifests.chromium,
            &host,
            "allowed_origins"
        ));
        assert!(manifest_is_owned(
            &manifests.firefox,
            &host,
            "allowed_extensions"
        ));
        let chromium: Value =
            serde_json::from_slice(&fs::read(&manifests.chromium).unwrap()).unwrap();
        assert_eq!(chromium["path"].as_str(), host.to_str());
        assert!(!chromium["path"].as_str().unwrap().starts_with(r"\\?\"));

        let entries = registration_entries(&manifests);
        assert_eq!(entries.len(), 7);
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.allowlist_field == "allowed_origins")
                .count(),
            6
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.allowlist_field == "allowed_extensions")
                .count(),
            1
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ownership_requires_the_current_install_host() {
        let (engine, root) = fixture();
        let host = expected_host(&engine, true).unwrap();
        let manifests = write_manifest_pair(&root, &host).unwrap();
        assert!(manifest_is_owned(
            &manifests.chromium,
            &host,
            "allowed_origins"
        ));
        assert!(!manifest_is_owned(
            &manifests.chromium,
            &root.join("OtherNativeHost.exe"),
            "allowed_origins"
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn path_matching_is_case_and_separator_insensitive() {
        assert!(same_path(
            Path::new(r"C:\Program Files\HLSDownloader\host.json"),
            Path::new(r"c:/program files/hlsdownloader/HOST.JSON")
        ));
        assert!(!same_path(
            Path::new(r"C:\Old\host.json"),
            Path::new(r"C:\New\host.json")
        ));
    }

    #[test]
    fn manifest_host_strips_windows_verbatim_prefixes() {
        assert_eq!(
            without_verbatim_prefix(Path::new(r"\\?\C:\Program Files\HLSDownloader\host.exe")),
            PathBuf::from(r"C:\Program Files\HLSDownloader\host.exe")
        );
        assert_eq!(
            without_verbatim_prefix(Path::new(r"\\?\UNC\server\share\host.exe")),
            PathBuf::from(r"\\server\share\host.exe")
        );
    }
}
