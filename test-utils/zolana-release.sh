# shellcheck shell=bash
# The pinned Zolana release and helpers that download and verify its files.
# Source this file from a bash script; do not execute it. Progress goes to
# stderr, so the callers keep stdout for their NAME=value lines.

if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
    echo "${BASH_SOURCE[0]} must be sourced, not executed" >&2
    exit 1
fi

zolana_release=cli-v0.10.0
# The cli-v0.10.0 release commit (357dd92) plus the fetchable gnark-sdk Go
# module; its SDK crates are the ones the release CLI was built from.
zolana_revision=ca4cf4f6f0c83c0b225342e4bc6f16b3da83eb08
# Digest of the release's SHA256SUMS, which pins the CLI binaries.
zolana_sha256sums_sha256=c60bebdff467f413e79a915bffd68bef15d6ca804afcd844f466a3a39727675e
# `zolana dev artifacts` does not fetch the user registry; this is the
# deployed user-registry v1.0.0 program.
user_registry_release=user-registry-v1.0.0
user_registry_sha256=cec2bce45f00d4693a8e3393b84d9b3a38e5aab08016a35b32164ebe0e03f43b

zolana_releases_url=https://github.com/helius-labs/zolana/releases/download

# zolana_platform: print the release platform name of this host.
zolana_platform() {
    case "$(uname -s)-$(uname -m)" in
        Darwin-arm64) echo darwin-arm64 ;;
        Linux-x86_64) echo linux-x64 ;;
        *)
            echo "No Zolana $zolana_release release for $(uname -s)-$(uname -m)" >&2
            return 1
            ;;
    esac
}

# check_sha256 <file> <sha256>: succeed when the file exists and matches.
check_sha256() {
    local digest
    [[ -f $1 ]] || return 1
    if command -v sha256sum >/dev/null 2>&1; then
        digest=$(sha256sum "$1") || return 1
    else
        digest=$(shasum -a 256 "$1") || return 1
    fi
    [[ ${digest%% *} == "$2" ]]
}

# fetch <url> <path> <sha256>: keep a file that already matches; otherwise
# download to a temporary file, verify it and move it into place.
fetch() {
    local url=$1 path=$2 sha256=$3 download
    if check_sha256 "$path" "$sha256"; then
        return 0
    fi
    echo "Downloading $url" >&2
    mkdir -p "$(dirname "$path")" || return 1
    download=$(mktemp "$path.download.XXXXXX") || return 1
    if ! curl -sSfL --retry 5 -o "$download" "$url"; then
        rm -f "$download"
        return 1
    fi
    if ! check_sha256 "$download" "$sha256"; then
        echo "$url does not match sha256 $sha256" >&2
        rm -f "$download"
        return 1
    fi
    chmod 644 "$download" && mv -f "$download" "$path"
}

# check_pins <Cargo.toml>: Cargo manifests cannot read zolana_revision; fail
# when any Zolana Git dependency is not pinned to it.
check_pins() {
    local repository=https://github.com/helius-labs/zolana
    if grep -F "$repository" "$1" | grep -F "git =" | grep -vqE "rev *= *\"$zolana_revision\""; then
        echo "$1 references $repository without pinning rev = \"$zolana_revision\"" >&2
        return 1
    fi
}

# install_zolana_cli <dir> <bin>: download the release CLI for this host to
# <dir>, check it against the pinned SHA256SUMS and install it as <bin>.
install_zolana_cli() {
    local dir=$1 bin=$2 platform asset cli_sha256
    local base="$zolana_releases_url/$zolana_release"
    platform=$(zolana_platform) || return 1
    asset="zolana-$platform-$zolana_release"
    fetch "$base/SHA256SUMS" "$dir/SHA256SUMS" "$zolana_sha256sums_sha256" || return 1
    cli_sha256=$(awk -v asset="$asset" '$2 == asset { print $1 }' "$dir/SHA256SUMS")
    if [[ -z $cli_sha256 ]]; then
        echo "$dir/SHA256SUMS lists no $asset" >&2
        return 1
    fi
    fetch "$base/$asset" "$dir/$asset" "$cli_sha256" || return 1
    mkdir -p "$(dirname "$bin")" && install -m 755 "$dir/$asset" "$bin"
}

# fetch_user_registry <path>: download the deployed user-registry program.
fetch_user_registry() {
    fetch "$zolana_releases_url/$user_registry_release/spr_user_registry.so" \
        "$1" "$user_registry_sha256"
}
