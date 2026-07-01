#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# artifact.sh — Storage interface for versioned build artifacts.
#
# Abstracts S3 (primary) + Bitbucket Downloads (visibility) so pipelines
# don't hardcode storage backends. Swap providers by editing this file only.
#
# Usage:
#   ./scripts/artifact.sh upload   <app> <tag> <dir>     Upload artifacts
#   ./scripts/artifact.sh download <app> <tag> <dir>     Download artifacts
#   ./scripts/artifact.sh promote  <app> <tag>           Copy tag → stable
#   ./scripts/artifact.sh latest   <app>                 Print latest tag
#   ./scripts/artifact.sh version  <app>                 Compute version tag
#
# Environment:
#   S3_ARTIFACT_BUCKET   — S3 bucket name (required)
#   AWS_DEFAULT_REGION   — AWS region (required)
#   BB_AUTH_STRING       — Bitbucket "user:app_password" (optional, for BB uploads)
#   BITBUCKET_REPO_OWNER — set automatically by Bitbucket Pipelines
#   BITBUCKET_REPO_SLUG  — set automatically by Bitbucket Pipelines
# ──────────────────────────────────────────────────────────────────────────────
set -euo pipefail

BUCKET="${S3_ARTIFACT_BUCKET:-planpal-artifacts}"
CMD="${1:-}"
APP="${2:-}"

# ── Helpers ──────────────────────────────────────────────────────────────────

die()  { echo "ERROR: $*" >&2; exit 1; }
info() { echo "==> $*"; }

require_var() {
  [ -n "${!1:-}" ] || die "Environment variable $1 is required"
}

# ── Commands ─────────────────────────────────────────────────────────────────

cmd_version() {
  # Compute version tag from project metadata + git commit
  local app="$1"
  local version commit

  if [ -f "Cargo.toml" ]; then
    version=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
  elif [ -f "package.json" ]; then
    version=$(node -p "require('./package.json').version")
  else
    die "Cannot detect version — no Cargo.toml or package.json found"
  fi

  commit=$(git rev-parse --short HEAD)
  echo "v${version}-${commit}"
}

cmd_upload() {
  local app="$1" tag="$2" dir="$3"
  require_var S3_ARTIFACT_BUCKET

  info "Uploading ${app}/${tag} to S3..."
  aws s3 cp "${dir}/" "s3://${BUCKET}/${app}/${tag}/" --recursive --quiet

  # Update latest marker
  echo -n "${tag}" | aws s3 cp - "s3://${BUCKET}/${app}/latest.txt"
  info "S3 upload complete: s3://${BUCKET}/${app}/${tag}/"

  # Bitbucket Downloads (best-effort, skip if no auth)
  if [ -n "${BB_AUTH_STRING:-}" ] && [ "${BB_AUTH_STRING:-}" != "\$BB_AUTH_STRING" ] && [ -n "${BITBUCKET_REPO_OWNER:-}" ]; then
    info "Uploading to Bitbucket Downloads..."
    local bb_url="https://api.bitbucket.org/2.0/repositories/${BITBUCKET_REPO_OWNER}/${BITBUCKET_REPO_SLUG}/downloads"
    for f in "${dir}"/*; do
      [ -f "$f" ] || continue
      local basename=$(basename "$f")
      # Prefix filename with tag for uniqueness
      cp "$f" "/tmp/${tag}-${basename}"
      curl -sS -X POST --user "${BB_AUTH_STRING}" "${bb_url}" \
        --form "files=@/tmp/${tag}-${basename}" || true
      rm -f "/tmp/${tag}-${basename}"
    done
    info "Bitbucket Downloads upload complete"
  else
    info "Skipping Bitbucket Downloads (BB_AUTH_STRING not set)"
  fi
}

cmd_download() {
  local app="$1" tag="$2" dir="$3"
  require_var S3_ARTIFACT_BUCKET

  # Resolve "latest" to actual tag
  if [ "${tag}" = "latest" ]; then
    tag=$(aws s3 cp "s3://${BUCKET}/${app}/latest.txt" -)
    info "Resolved 'latest' to ${tag}"
  fi

  info "Downloading ${app}/${tag} from S3..."
  mkdir -p "${dir}"
  aws s3 cp "s3://${BUCKET}/${app}/${tag}/" "${dir}/" --recursive --quiet
  info "Downloaded to ${dir}/"
}

cmd_promote() {
  local app="$1" tag="$2"
  require_var S3_ARTIFACT_BUCKET

  # Resolve "latest" to actual tag
  if [ "${tag}" = "latest" ]; then
    tag=$(aws s3 cp "s3://${BUCKET}/${app}/latest.txt" -)
    info "Resolved 'latest' to ${tag}"
  fi

  local version_prefix
  version_prefix=$(echo "${tag}" | sed 's/-[a-f0-9]*$//')

  info "Promoting ${app}/${tag} → ${app}/${version_prefix}-stable"

  # Delete old stable, copy new
  aws s3 rm "s3://${BUCKET}/${app}/${version_prefix}-stable/" --recursive --quiet 2>/dev/null || true
  aws s3 cp "s3://${BUCKET}/${app}/${tag}/" "s3://${BUCKET}/${app}/${version_prefix}-stable/" --recursive --quiet

  # Also update a stable.txt marker
  echo -n "${tag}" | aws s3 cp - "s3://${BUCKET}/${app}/stable.txt"

  info "Promotion complete: ${version_prefix}-stable → ${tag}"
}

cmd_latest() {
  local app="$1"
  require_var S3_ARTIFACT_BUCKET
  aws s3 cp "s3://${BUCKET}/${app}/latest.txt" - 2>/dev/null || echo "(none)"
}

# ── Dispatch ─────────────────────────────────────────────────────────────────

case "${CMD}" in
  version)
    [ -n "${APP}" ] || die "Usage: $0 version <app>"
    cmd_version "${APP}"
    ;;
  upload)
    TAG="${3:-}"; DIR="${4:-}"
    [ -n "${APP}" ] && [ -n "${TAG}" ] && [ -n "${DIR}" ] || die "Usage: $0 upload <app> <tag> <dir>"
    cmd_upload "${APP}" "${TAG}" "${DIR}"
    ;;
  download)
    TAG="${3:-}"; DIR="${4:-}"
    [ -n "${APP}" ] && [ -n "${TAG}" ] && [ -n "${DIR}" ] || die "Usage: $0 download <app> <tag> <dir>"
    cmd_download "${APP}" "${TAG}" "${DIR}"
    ;;
  promote)
    TAG="${3:-latest}"
    [ -n "${APP}" ] || die "Usage: $0 promote <app> [tag]"
    cmd_promote "${APP}" "${TAG}"
    ;;
  latest)
    [ -n "${APP}" ] || die "Usage: $0 latest <app>"
    cmd_latest "${APP}"
    ;;
  *)
    echo "Usage: $0 {version|upload|download|promote|latest} <app> [args...]"
    exit 1
    ;;
esac
