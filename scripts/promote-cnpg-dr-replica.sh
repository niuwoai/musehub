#!/usr/bin/env bash
# Promote a preconfigured CloudNativePG replica after the old region is fenced
# and the old cluster has been deliberately demoted.
set -euo pipefail

NAMESPACE="${MUSEHUB_DR_NAMESPACE:-musehub}"
OLD_CONTEXT="${MUSEHUB_DR_OLD_KUBECTL_CONTEXT:?set the old-region kubectl context}"
NEW_CONTEXT="${MUSEHUB_DR_NEW_KUBECTL_CONTEXT:?set the DR-region kubectl context}"
OLD_CLUSTER="${MUSEHUB_DR_OLD_CLUSTER:?set the current primary CloudNativePG Cluster name}"
NEW_CLUSTER="${MUSEHUB_DR_NEW_CLUSTER:?set the DR replica CloudNativePG Cluster name}"

[[ "$OLD_CLUSTER" != "$NEW_CLUSTER" ]] || {
  echo "old and new cluster names must differ" >&2
  exit 2
}
[[ "${MUSEHUB_DR_OLD_PRIMARY_FENCED:-}" == "YES" ]] || {
  echo "fence the old region and set MUSEHUB_DR_OLD_PRIMARY_FENCED=YES only after fencing is verified" >&2
  exit 1
}
command -v kubectl >/dev/null 2>&1 || { echo "kubectl is required" >&2; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "jq is required" >&2; exit 1; }

old_spec="$(kubectl --context "$OLD_CONTEXT" get cluster "$OLD_CLUSTER" --namespace "$NAMESPACE" -o json)"
new_spec="$(kubectl --context "$NEW_CONTEXT" get cluster "$NEW_CLUSTER" --namespace "$NAMESPACE" -o json)"
demotion_token="$(jq -er '.status.demotionToken | select(type == "string" and length > 0)' <<<"$old_spec")" || {
  echo "old cluster has no demotion token; stop and inspect its demotion status" >&2
  exit 1
}
old_source="$(jq -r '.spec.replica.source // empty' <<<"$old_spec")"
old_primary="$(jq -r '.spec.replica.primary // empty' <<<"$old_spec")"
new_source="$(jq -r '.spec.replica.source // empty' <<<"$new_spec")"

[[ "$old_source" == "$NEW_CLUSTER" && "$old_primary" == "$NEW_CLUSTER" ]] || {
  echo "old cluster is not configured as a replica of the intended DR cluster" >&2
  exit 1
}
[[ "$new_source" == "$OLD_CLUSTER" ]] || {
  echo "DR cluster source does not match the demoted old primary" >&2
  exit 1
}

jq -cn --arg cluster "$NEW_CLUSTER" --arg token "$demotion_token" \
  '{spec:{replica:{primary:$cluster,promotionToken:$token}}}' \
  | kubectl --context "$NEW_CONTEXT" patch cluster "$NEW_CLUSTER" \
      --namespace "$NAMESPACE" --type=merge --patch-file=/dev/stdin >/dev/null

printf 'cnpg_dr_promotion_requested=ok namespace=%s old_cluster=%s new_cluster=%s\n' \
  "$NAMESPACE" "$OLD_CLUSTER" "$NEW_CLUSTER"
printf 'Verify both clusters with: kubectl cnpg status %s --namespace %s\n' \
  "$NEW_CLUSTER" "$NAMESPACE"
printf 'Then verify the writer endpoint, former-primary fencing, and every API with scripts/verify-postgres-primary.sh.\n'
