name := `cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].name'`
version := `cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version'`

build:
    #!/usr/bin/env bash
    set -euo pipefail
    VERSION=$(cargo metadata --no-deps --format-version 1 | python3 -c "import sys,json; print(json.load(sys.stdin)['packages'][0]['version'])")
    echo "Building 👷 watchmedo v${VERSION}..."
    mkdir -p dist
    cargo build --workspace --all --release
    cargo metadata --no-deps --format-version 1 \
        | jq -r '.packages[].targets[] | select(.kind[] == "bin") | .name' \
        | while read bin; do cp "target/release/$bin" "dist/$bin-v${VERSION}"; done
    echo "Done ✅"

test:
	cargo test

prepare-release level='patch':
    cargo-release release --execute {{level}}

gh-release level='patch': (prepare-release level)
    #!/usr/bin/env bash
    set -euo pipefail
    VERSION=$(cargo metadata --no-deps --format-version 1 | python3 -c "import sys,json; print(json.load(sys.stdin)['packages'][0]['version'])")
    just build
    gh release create "v${VERSION}" \
        "./dist/watchmedo-v${VERSION}" \
        "./dist/watchmedo-sidecar-v${VERSION}" \
        --generate-notes

