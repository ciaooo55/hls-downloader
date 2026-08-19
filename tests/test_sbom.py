from scripts.generate_sbom import build_sbom


def test_sbom_contains_locked_python_node_rust_and_ffmpeg_components():
    payload = build_sbom("3.0.10")
    purls = {item["purl"] for item in payload["components"]}

    assert payload["bomFormat"] == "CycloneDX"
    assert payload["specVersion"] == "1.6"
    assert payload["metadata"]["component"]["version"] == "3.0.10"
    assert any(value.startswith("pkg:pypi/fastapi@") for value in purls)
    assert any(value.startswith("pkg:npm/react@") for value in purls)
    assert any(value.startswith("pkg:cargo/tauri@") for value in purls)
    assert "pkg:generic/ffmpeg@N-126217-ge1e325235e" in purls
