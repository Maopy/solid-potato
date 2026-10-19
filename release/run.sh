#!/bin/sh
set -eu
package_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec "$package_dir/live-archive-core" moderation serve --config "$package_dir/config/unified.yml"
