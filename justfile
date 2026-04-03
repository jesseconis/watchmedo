name := `cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].name'`
version := `cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version'`

build:
	@echo "Building 👷  {{name}} v{{version}}..."
	mkdir -p dist
	cargo build --workspace --all --release
	cargo metadata --no-deps --format-version 1 \
		| jq -r '.packages[].targets[] | select(.kind[] == "bin") | .name' \
		| while read bin; do cp "target/release/$bin" "dist/$bin-v{{version}}"; done
	@echo "Done ✅"

test:
	cargo test

prepare-release level='patch':
    cargo-release release --execute {{level}}

gh-release level='patch': (prepare-release level)
    #!/usr/bin/env bash
    just build
    git fetch --tags
    VERSION=$(cargo metadata --no-deps --format-version 1 | python3 -c "import sys,json; print(json.load(sys.stdin)['packages'][0]['version'])")
    gh release create "v${VERSION}" \
        "./dist/{{name}}-v${VERSION}" \
        --generate-notes

