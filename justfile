build:
    #!/usr/bin/env bash
    set -euo pipefail
    VERSION=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "watchmedo") | .version')
    echo "Building 👷 watchmedo v${VERSION}..."
    mkdir -p dist
    cargo build --workspace --all --release -v
    cargo metadata --no-deps --format-version 1 \
        | jq -r '.packages[].targets[] | select(.kind[] == "bin") | .name' \
        | while read bin; do cp "target/release/$bin" "dist/$bin-v${VERSION}"; done
    echo "Done ✅"

gh-release level='patch': (prepare-release level)
    #!/usr/bin/env bash
    set -euo pipefail
    VERSION=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "watchmedo") | .version')
    just build
    gh release create "v${VERSION}" \
        "./dist/watchmedo-v${VERSION}" \
        "./dist/watchmedo-sidecar-v${VERSION}" \
        --generate-notes
test:
	cargo test

prepare-release level='patch':
    cargo-release release --manifest-path ./Cargo.toml -v --workspace --execute {{level}}
