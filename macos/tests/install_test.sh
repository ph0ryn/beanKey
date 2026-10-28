#!/bin/bash
set -euo pipefail

root=$(mktemp -d "${TMPDIR:-/tmp}/beankey-install-test.XXXXXX")
root=$(cd "$root" && pwd -P)
trap '[[ "$root" == /*/beankey-install-test.* ]] && rm -r -- "$root"' EXIT
package="$root/package"
app="$package/Library/Input Methods/beanKey.app"
mkdir -p "$package/bin" "$(dirname "$app")"
/usr/bin/ditto "$2" "$app"
mkdir -p "$app/Contents/Resources/beankey/bin"
cp /usr/bin/true "$app/Contents/Resources/beankey/bin/beankey-daemon"
# Do not register a real input source during the test.
ln -s /usr/bin/true "$package/bin/beankey-register-input-method"
sed 's/@BEANKEY_NIX_STORE@//g' "$1" > "$package/bin/beankey-install"

printf '[conversion]\ntype_backslash = true\n' > "$root/settings.toml"
for scenario in new existing symlink dangling; do
  home="$root/$scenario"
  config="$home/Library/Application Support/beanKey/config.toml"
  mkdir -p "$(dirname "$config")"
  cp "$root/settings.toml" "$home/settings.toml"
  case "$scenario" in
    existing) cp "$home/settings.toml" "$config" ;;
    symlink) ln -s "$home/settings.toml" "$config" ;;
    dangling) ln -s "$home/missing.toml" "$config" ;;
  esac
  HOME="$home" /bin/bash "$package/bin/beankey-install"
  case "$scenario" in
    new)
      test -f "$config"
      awk '!/^[[:space:]]*(#|$)/ { print "Installer must delegate defaults to the daemon"; exit 1 }' "$config"
      test "$(/usr/bin/stat -f %Lp "$config")" = 644
      ;;
    existing) cmp "$root/settings.toml" "$config" ;;
    symlink)
      test "$(readlink "$config")" = "$home/settings.toml"
      cmp "$root/settings.toml" "$config"
      ;;
    dangling)
      test "$(readlink "$config")" = "$home/missing.toml"
      test ! -e "$config"
      ;;
  esac
done
