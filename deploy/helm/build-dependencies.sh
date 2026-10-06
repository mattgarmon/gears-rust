#!/usr/bin/env bash
set -euo pipefail

helm_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
platform_chart="$helm_root/toolkit-platform"

for chart in "$helm_root"/*; do
  [[ -f "$chart/Chart.yaml" ]] || continue
  [[ "$chart" == "$platform_chart" || "$chart" == "$helm_root/toolkit-common" ]] && continue
  helm dependency update "$chart"
done

helm dependency update "$platform_chart"