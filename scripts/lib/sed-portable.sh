#!/usr/bin/env bash
# GNU sed uses `sed -i`; BSD/macOS sed requires `sed -i ''`.
sed_inplace() {
  if [[ "$(uname -s)" == Darwin ]]; then
    sed -i '' "$@"
  else
    sed -i "$@"
  fi
}
