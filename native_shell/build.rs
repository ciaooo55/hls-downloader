fn main() {
    println!("cargo:rustc-check-cfg=cfg(native_host_offer_v2)");
    println!("cargo:rerun-if-changed=../native_clients/src/native_host.rs");

    let Ok(raw) = std::fs::read_to_string("../native_clients/src/native_host.rs") else {
        return;
    };
    let normalized = raw.replace("\r\n", "\n");
    let legacy_unupdated_offer =
        "payload.get(\"size\").and_then(Value::as_u64).unwrap_or(0)\n        },\n    })";
    if !normalized.contains(legacy_unupdated_offer) {
        println!("cargo:rustc-cfg=native_host_offer_v2");
    }
}
