"""Инструменты GitHub Actions для подписанного desktop-релиза."""

import base64
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.request
import zipfile
from pathlib import Path


# SHA-256 опубликованных архивов из официальных релизов Tauri и Dioxus.
CLI_RELEASES = {
    "cargo-tauri": (
        "tauri-apps/tauri", "tauri-cli-v2.12.6",
        {
            "x86_64-pc-windows-msvc": "39636fa68bc00f13dd610fb57688f292e93742978cc35395f2f1be7c540b88a1",
            "aarch64-apple-darwin": "1c71c48a9aea9616a857c92bab63bc9bce113e348929493ba0bbaa4d3f4ce4c3",
            "x86_64-apple-darwin": "dd318ab5235a910629fe1e18560b67bd4949618deb2a4b39458a887c593524d1",
        },
    ),
    "dx": (
        "DioxusLabs/dioxus", "v0.7.10",
        {
            "x86_64-pc-windows-msvc": "45eb4f87b7f86fdba8508ee7e0ef1f9e13cf8f2ffcc6a2d5858d0e8ea7dd1531",
            "aarch64-apple-darwin": "4aff37aea6b8fbce7bee3f8bbbea1bd71f60e0fa09537f35759f6494da861e37",
            "x86_64-apple-darwin": "b2751a700b8b02f85fcdd8fe6e63c20876b4af9f0921c5be4c703e6f872b67ee",
        },
    ),
}


def tag_name():
    tag = os.environ["GITHUB_REF_NAME"]
    if os.environ["GITHUB_REF_TYPE"] != "tag" or not re.fullmatch(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", tag):
        raise ValueError("Выберите существующий тег vX.Y.Z при ручном запуске.")
    return tag


def check_version():
    import tomllib

    tag = tag_name()
    workspace = tomllib.loads(Path("Cargo.toml").read_text(encoding="utf-8"))
    config = json.loads(Path("src-tauri/tauri.conf.json").read_text(encoding="utf-8"))
    if workspace["workspace"]["package"]["version"] != tag[1:] or config["version"] != tag[1:]:
        raise ValueError("Версии Cargo и Tauri должны совпадать с тегом.")
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    expected = subprocess.check_output(["git", "rev-parse", os.environ["GITHUB_SHA"] + "^{commit}"], text=True).strip()
    if head != expected:
        raise ValueError("Checkout не совпадает с коммитом запуска.")


def digest(file):
    with file.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def install_tools():
    windows = sys.platform == "win32"
    machine = platform.machine().lower()
    arch = "aarch64" if machine in ("aarch64", "arm64") else "x86_64"
    triple = f"{arch}-pc-windows-msvc" if windows else f"{arch}-apple-darwin"
    cargo_bin = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")) / "bin"
    cargo_bin.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        for name, (repo, release, hashes) in CLI_RELEASES.items():
            zipname = f"{name}-{triple}.zip"
            archive = Path(tmp, zipname)
            urllib.request.urlretrieve(f"https://github.com/{repo}/releases/download/{release}/{zipname}", archive)
            if digest(archive) != hashes[triple]:
                raise ValueError(f"SHA-256 не совпал: {zipname}")
            binary = name + (".exe" if windows else "")
            with zipfile.ZipFile(archive) as bundle:
                member, = [info for info in bundle.infolist() if Path(info.filename).name == binary]
                with bundle.open(member) as src, (cargo_bin / binary).open("wb") as dest:
                    shutil.copyfileobj(src, dest)
            (cargo_bin / binary).chmod(0o755)
            subprocess.run([str(cargo_bin / binary), "--version"], check=True)


def configure():
    pubkey = os.environ["TAURI_UPDATER_PUBKEY"].strip()
    if not pubkey or not os.environ.get("TAURI_SIGNING_PRIVATE_KEY"):
        raise ValueError("Укажите публичный и закрытый ключи Tauri updater.")
    bundle = {"createUpdaterArtifacts": True}
    if sys.platform == "win32":
        bundle["windows"] = {
            "certificateThumbprint": os.environ["WINDOWS_CERTIFICATE_THUMBPRINT"],
            "digestAlgorithm": "sha256",
            "timestampUrl": "http://timestamp.digicert.com",
            "tsp": True,
        }
    else:
        identity = os.environ["APPLE_SIGNING_IDENTITY"]
        for key in ("APPLE_ID", "APPLE_TEAM_ID", "APPLE_PASSWORD", "APPLE_CERTIFICATE", "APPLE_CERTIFICATE_PASSWORD"):
            if not os.environ.get(key):
                raise ValueError(f"Не задан {key}.")
        if not identity.startswith("Developer ID Application: "):
            raise ValueError("Требуется сертификат Developer ID Application.")
        bundle["macOS"] = {"signingIdentity": identity}
    config = {"plugins": {"updater": {"pubkey": pubkey}}, "bundle": bundle}
    Path(os.environ["RUNNER_TEMP"], "studio-release.json").write_text(json.dumps(config), encoding="utf-8")


def asset_names():
    prefix = f"Koteyye-Media-Studio-{tag_name()[1:]}-"
    return prefix + "windows-x64-setup.exe", prefix + "macos-universal.app.tar.gz", prefix + "macos-universal.dmg"


def stage(platform_name):
    win, mac, dmg = asset_names()
    if platform_name == "windows":
        bundle = Path("target/release/bundle")
        files = [("nsis/*-setup.exe", win), ("nsis/*-setup.exe.sig", win + ".sig")]
    else:
        bundle = Path("target/universal-apple-darwin/release/bundle")
        files = [("macos/*.app.tar.gz", mac), ("macos/*.app.tar.gz.sig", mac + ".sig"), ("dmg/*.dmg", dmg)]
    dest = Path("release-stage")
    dest.mkdir(exist_ok=True)
    for pattern, name in files:
        source, = bundle.glob(pattern)
        shutil.copyfile(source, dest / name)


def verify_manifest():
    tag = tag_name()
    root = Path("release-stage")
    win, mac, dmg = asset_names()
    if not (root / dmg).stat().st_size:
        raise ValueError("Пустой образ DMG.")
    platforms = {}
    with tempfile.TemporaryDirectory() as tmp:
        key_file = Path(tmp, "updater.pub")
        key_file.write_bytes(base64.b64decode(os.environ["TAURI_UPDATER_PUBKEY"].strip(), validate=True))
        for filename, targets in [(win, ["windows-x86_64"]), (mac, ["darwin-aarch64", "darwin-x86_64"])]:
            payload = root / filename
            signature = (root / (filename + ".sig")).read_text(encoding="ascii").strip()
            sig_file = Path(tmp, "release.minisig")
            sig_file.write_bytes(base64.b64decode(signature, validate=True))
            subprocess.run(["minisign", "-Vm", str(payload), "-p", str(key_file), "-x", str(sig_file)], check=True)
            signed_comment, = (line for line in sig_file.read_text(encoding="utf-8").splitlines() if line.startswith("trusted comment: "))
            if signed_comment.split("\t")[-1] != "version:" + tag[1:]:
                raise ValueError(f"В подписанном комментарии {filename} отсутствует версия {tag[1:]}.")
            url = f"{os.environ['GITHUB_SERVER_URL']}/{os.environ['GITHUB_REPOSITORY']}/releases/download/{tag}/{filename}"
            for target in targets:
                platforms[target] = {"url": url, "signature": signature}
    manifest = {"version": tag[1:], "platforms": platforms}
    (root / "latest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


def gh_json(*args):
    return json.loads(subprocess.check_output(["gh", "api", *args], text=True, encoding="utf-8"))


def draft_release():
    tag = tag_name()
    repo = os.environ["GITHUB_REPOSITORY"]
    endpoint = f"repos/{repo}/releases"

    # Тег мог переместиться за время notarization; артефакты принадлежат одному коммиту.
    obj = gh_json(f"repos/{repo}/git/ref/tags/{tag}")["object"]
    while obj["type"] == "tag":
        obj = gh_json(f"repos/{repo}/git/tags/{obj['sha']}")["object"]
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    if obj["type"] != "commit" or obj["sha"] != commit:
        raise ValueError("Тег был перемещён после запуска сборки.")

    matches = [r for page in gh_json("--paginate", "--slurp", endpoint + "?per_page=100") for r in page if r["tag_name"] == tag]
    if not matches:
        subprocess.run(["gh", "release", "create", tag, "--repo", repo, "--draft", "--verify-tag", "--generate-notes",
                        "--title", f"Koteyye Media Studio {tag[1:]}"], check=True)
    elif len(matches) != 1 or not matches[0]["draft"]:
        raise ValueError("Релиз этого тега уже опубликован.")
    win, mac, dmg = asset_names()
    names = [win, win + ".sig", mac, mac + ".sig", dmg, "latest.json"]
    files = [Path("release-stage", name) for name in names]
    subprocess.run(["gh", "release", "upload", tag, "--repo", repo, "--clobber", *map(str, files)], check=True)
    details = json.loads(subprocess.check_output(
        ["gh", "release", "view", tag, "--repo", repo, "--json", "databaseId,isDraft"], text=True,
    ))
    if not details["isDraft"]:
        raise ValueError("Релиз больше не является черновиком.")
    assets = {a["name"]: a for page in gh_json("--paginate", "--slurp", f"{endpoint}/{details['databaseId']}/assets") for a in page}
    for file in files:
        if assets[file.name].get("digest") != "sha256:" + digest(file):
            raise ValueError(f"Контрольная сумма загруженного файла {file.name} не совпадает.")


if __name__ == "__main__":
    {
        "check-version": check_version,
        "install-tools": install_tools,
        "configure": configure,
        "stage": lambda: stage(sys.argv[2]),
        "verify-manifest": verify_manifest,
        "draft-release": draft_release,
    }[sys.argv[1]]()
