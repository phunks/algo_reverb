set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

root_dir := justfile_directory()
nice_plug_rev := "faa0cd61d249bf7c5103f3072be0c65cf72e9f4e"
egui_knob_rev := "12f5c467a236dd6281db6d127c8359aa108d39ee"

# Show available commands
default:
    @just --list

# Fetch nice-plug at the revision used for nice-plug-egui 0.6.0.
setup-nice-plug:
    @just --justfile "{{ justfile() }}" setup-target \
        "ext/nice-plug" \
        "ext/nice-plug/crates/nice-plug-egui" \
        "{{ nice_plug_rev }}" \
        "https://codeberg.org/RustAudio/nice-plug.git" \
        "patches/nice-plug-egui-param-knob.patch" \
        ""

# Fetch egui_knob at the revision used for version 0.6.3.
setup-egui-knob:
    @just --justfile "{{ justfile() }}" setup-target \
        "ext/egui_knob" \
        "ext/egui_knob" \
        "{{ egui_knob_rev }}" \
        "https://github.com/obsqrbtz/egui_knob.git" \
        "patches/egui-knob-egui-0.37.patch" \
        ""

# Backwards-compatible alias for the old, misleading recipe name.
setup-qtbridge-rust: setup-nice-plug

# target_patch is optional and must use paths relative to the cloned repository root.
setup-target clone_dir target_dir repo_rev repo_url target_patch sparse_dirs:
    mkdir -p "{{ root_dir }}/ext"
    if [ ! -d "{{ root_dir }}/{{ clone_dir }}/.git" ]; then \
      git -c core.autocrlf=false clone --depth 1 --filter=blob:none "{{ repo_url }}" "{{ root_dir }}/{{ clone_dir }}"; \
    else \
      echo "{{ clone_dir }} already exists; skipping clone"; \
    fi
    cd "{{ root_dir }}/{{ clone_dir }}" && git fetch --depth 1 origin "{{ repo_rev }}"
    if [ -n "{{ sparse_dirs }}" ]; then \
      cd "{{ root_dir }}/{{ clone_dir }}" && git sparse-checkout init --cone; \
      cd "{{ root_dir }}/{{ clone_dir }}" && git sparse-checkout set {{ sparse_dirs }}; \
    fi
    cd "{{ root_dir }}/{{ clone_dir }}" && \
      if [ "$(git rev-parse HEAD)" != "$(git rev-parse FETCH_HEAD)" ]; then \
        if [ -n "$(git status --porcelain)" ]; then \
          echo "Refusing to change revisions with local changes in {{ clone_dir }}" >&2; exit 1; \
        fi; \
        git -c core.autocrlf=false checkout --detach FETCH_HEAD; \
      else \
        git -c core.autocrlf=false checkout --detach FETCH_HEAD; \
      fi
    test -d "{{ root_dir }}/{{ target_dir }}"
    @just --justfile "{{ justfile() }}" apply-target-patch "{{ clone_dir }}" "{{ target_patch }}"

# DESTRUCTIVE: discard tracked changes and untracked files in the selected clone.
reset-target clone_dir target_dir repo_rev target_patch:
    test -d "{{ root_dir }}/{{ clone_dir }}/.git"
    cd "{{ root_dir }}/{{ clone_dir }}" && git fetch --depth 1 origin "{{ repo_rev }}"
    cd "{{ root_dir }}/{{ clone_dir }}" && git reset --hard FETCH_HEAD
    cd "{{ root_dir }}/{{ clone_dir }}" && git clean -fd
    test -d "{{ root_dir }}/{{ target_dir }}"
    @just --justfile "{{ justfile() }}" apply-target-patch "{{ clone_dir }}" "{{ target_patch }}"

# Apply a patch once; fail without modifying files if it conflicts.
apply-target-patch clone_dir target_patch:
    if [ -n "{{ target_patch }}" ]; then \
      cd "{{ root_dir }}/{{ clone_dir }}"; \
      if git apply --reverse --check "{{ root_dir }}/{{ target_patch }}" 2>/dev/null; then \
        echo "{{ target_patch }} already applied; skipping"; \
      else \
        git apply --check "{{ root_dir }}/{{ target_patch }}"; \
        git apply "{{ root_dir }}/{{ target_patch }}"; \
      fi; \
    fi

setup-ext: setup-nice-plug setup-egui-knob

# Build a self-contained macOS app (optionally specify a Rust target triple).
bundle-macos target="":
    bash "{{ root_dir }}/scripts/package-macos.sh" "{{ target }}"
