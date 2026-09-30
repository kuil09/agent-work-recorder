#!/usr/bin/env python3
"""Identify built binaries and export an immutable skill copy with source metadata.

Fingerprints identify local artifacts, not signed release provenance. No capture runs.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SKILL = ROOT / 'skills/agent-work-recorder'


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def skill_files(directory):
    return {str(path.relative_to(directory)): digest(path)
            for path in sorted(directory.rglob('*')) if path.is_file() and path.name != 'BUILD.json'}


def source_revision():
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    dirty = subprocess.run(['git', 'diff', '--quiet', 'HEAD'], cwd=ROOT, check=False).returncode
    return revision + ('-dirty' if dirty else '')


def rust_source_id():
    paths = [Path('Cargo.toml'), Path('Cargo.lock'), Path('build.rs')]
    paths += [path.relative_to(ROOT) for path in (ROOT / 'src').glob('*.rs')]
    data = b''.join(str(path).encode() + b'\0' + (ROOT / path).read_bytes() + b'\0' for path in sorted(paths))
    return subprocess.check_output(['git', 'hash-object', '--stdin'], input=data, cwd=ROOT).decode().strip()


def skill_manifest():
    files = skill_files(SKILL)
    revision = source_revision()
    # Revision participates in the package identity: unchanged skill text in a
    # later commit must not reuse an earlier commit's BUILD.json.
    fingerprint = hashlib.sha256(json.dumps({'revision': revision, 'files': files}, sort_keys=True).encode()).hexdigest()
    return {'schema': 1, 'name': 'agent-work-recorder', 'revision': revision,
            'sha256': fingerprint, 'files': files}


def package_skill(output_root):
    metadata = skill_manifest()
    destination = output_root / metadata['sha256'] / 'agent-work-recorder'
    if destination.exists():
        recorded = json.loads((destination / 'BUILD.json').read_text())
        if recorded != metadata or skill_files(destination) != metadata['files']:
            raise ValueError(f'Existing package was changed; preserved at {destination}')
        return destination
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.skill-', dir=destination.parent) as tmp:
        staging = Path(tmp) / 'agent-work-recorder'
        shutil.copytree(SKILL, staging)
        (staging / 'BUILD.json').write_text(json.dumps(metadata, indent=2) + '\n')
        staging.rename(destination)
    return destination


def write_manifest(rec, helper, output):
    version = subprocess.check_output([str(rec.resolve()), '--version'], text=True).strip()
    source = rust_source_id()
    revision = source_revision()
    if f'revision {revision}; source {source}' not in version:
        raise ValueError('rec build does not match current source; rebuild before writing the manifest')
    # Make's prerequisites rebuild both executables before this metadata is written.
    metadata = {'schema': 1, 'revision': revision, 'rust_source': source,
                'rec_version': version, 'binaries': {'rec': digest(rec), 'rec-capture': digest(helper)},
                'skill': skill_manifest()}
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode='w', dir=output.parent, delete=False) as tmp:
        json.dump(metadata, tmp, indent=2); tmp.write('\n')
        staging = Path(tmp.name)
    staging.replace(output)


def verify(bin_dir, skill_dir):
    manifest = json.loads((bin_dir / 'agent-work-recorder-build.json').read_text())
    if manifest.get('schema') != 1 or set(manifest['binaries']) != {'rec', 'rec-capture'}:
        raise ValueError('Unsupported installation manifest')
    if manifest['rust_source'] != rust_source_id() or manifest['skill']['files'] != skill_files(SKILL):
        raise ValueError('Installation does not match current repository source; rebuild and update the skill package')
    for name, expected in manifest['binaries'].items():
        if digest(bin_dir / name) != expected:
            raise ValueError(f'{name} differs from its installation manifest; rebuild/reinstall the pair')
    if skill_files(skill_dir) != manifest['skill']['files']:
        raise ValueError('Installed skill differs from the binary package; preserve edits before updating it')
    installed = json.loads((skill_dir / 'BUILD.json').read_text())
    if installed != manifest['skill']:
        raise ValueError('Installed skill BUILD.json does not match its files and installation manifest')
    print(json.dumps({'verified': True, 'revision': manifest['revision'], 'skill_sha256': installed['sha256']}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    skill = commands.add_parser('skill')
    skill.add_argument('--output-root', type=Path, default=ROOT / 'target/skills')
    manifest = commands.add_parser('manifest')
    manifest.add_argument('--rec', type=Path, required=True)
    manifest.add_argument('--helper', type=Path, required=True)
    manifest.add_argument('--output', type=Path, required=True)
    check = commands.add_parser('verify')
    check.add_argument('--bin-dir', type=Path, required=True)
    check.add_argument('--skill-dir', type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == 'skill': print(package_skill(args.output_root))
        elif args.command == 'manifest': write_manifest(args.rec, args.helper, args.output)
        else: verify(args.bin_dir, args.skill_dir)
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f'{error}\n')


if __name__ == '__main__': main()
