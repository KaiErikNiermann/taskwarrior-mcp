set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# List recipes
default:
  @just --list

# Build release binary
build:
  cargo build --release

# Run all tests
test:
  cargo test

# Run clippy + fmt check
lint:
  cargo clippy -- -D warnings
  cargo fmt --check

# Install dev tooling (cargo-edit for version bumping)
setup:
  cargo install cargo-edit

# Bump version (patch/minor/major), build, release to GitHub, and update server.json
release bump="patch":
  @bump="{{bump}}"; \
    if [[ "$bump" == bump=* ]]; then bump="${bump#bump=}"; fi; \
    cargo set-version --bump "$bump"
  @version=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version'); \
    just _release "$version"

# Release an explicit version
release-version version:
  @version="{{version}}"; \
    if [[ "$version" == version=* ]]; then version="${version#version=}"; fi; \
    cargo set-version "$version"; \
    just _release "$version"

# Re-trigger publish for an existing version by re-tagging HEAD
rerun version:
  @version="{{version}}"; \
    if [[ "$version" == version=* ]]; then version="${version#version=}"; fi; \
    git push; \
    git tag -d v"$version" || true; \
    git push --delete origin v"$version" || true; \
    git tag v"$version"; \
    git push origin v"$version"

# Delete and recreate the GitHub release + retag HEAD at the same version
rerelease version:
  @version="{{version}}"; \
    if [[ "$version" == version=* ]]; then version="${version#version=}"; fi; \
    gh release delete v"$version" -y || true; \
    just rerun "$version"; \
    gh release create v"$version" --title "v$version" --generate-notes

# Publish current server.json to the MCP registry (requires: mcp-publisher on PATH, already logged in)
publish:
  mcp-publisher publish

# Internal: build, upload mcpb to GitHub, update server.json, commit, tag
_release version:
  @version="{{version}}"; \
    just build; \
    mcpb="task-warrior-mcp.mcpb"; \
    cp target/release/task-warrior-mcp "$mcpb"; \
    sha256=$(openssl dgst -sha256 "$mcpb" | awk '{print $2}'); \
    repo_url=$(gh repo view --json url -q .url); \
    artifact_url="$repo_url/releases/download/v${version}/task-warrior-mcp.mcpb"; \
    jq --arg v "$version" --arg sha "$sha256" --arg url "$artifact_url" \
        --arg repo "$repo_url" --arg rname "io.github.$(gh api user -q .login)/task-warrior-mcp" \
        '.version = $v | .name = $rname | .repository.url = $repo | .packages[0].identifier = $url | .packages[0].fileSha256 = $sha' \
        server.json > server.json.tmp && mv server.json.tmp server.json; \
    git add Cargo.toml Cargo.lock server.json; \
    git commit -m "chore(release): v$version"; \
    git push; \
    git tag v"$version"; \
    git push origin v"$version"; \
    gh release create v"$version" --title "v$version" --generate-notes "$mcpb"; \
    rm -f "$mcpb"; \
    echo ""; \
    echo "Released v$version. Run 'just publish' to push to the MCP registry."
