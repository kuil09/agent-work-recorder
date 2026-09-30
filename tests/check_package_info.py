#!/usr/bin/env python3
"""Verify artifact identity. Helper fixture bytes are never executed as capture."""
import importlib.util
import json
from pathlib import Path
import shutil
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('package_info', ROOT / 'tools/package_info.py')
package_info = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package_info)


def rejected(action, text):
    try:
        action()
    except ValueError as error:
        assert text in str(error), str(error)
    else:
        raise AssertionError('expected mismatch rejection')


def main():
    with tempfile.TemporaryDirectory(prefix='rec-package-info-') as temporary:
        root = Path(temporary)
        skill = package_info.package_skill(root / 'packages')
        assert package_info.package_skill(root / 'packages') == skill
        metadata = json.loads((skill / 'BUILD.json').read_text())
        assert metadata['files'] == package_info.skill_files(skill)
        revision_function = package_info.source_revision
        try:
            package_info.source_revision = lambda: 'later-test-revision'
            later = package_info.package_skill(root / 'packages')
            assert later != skill
            assert json.loads((later / 'BUILD.json').read_text())['revision'] == 'later-test-revision'
        finally:
            package_info.source_revision = revision_function
        bins = root / 'bin'; bins.mkdir()
        shutil.copy2(ROOT / 'target/debug/rec', bins / 'rec')
        helper = bins / 'rec-capture'
        helper.write_bytes(b'non-executable helper metadata fixture')
        package_info.write_manifest(bins / 'rec', helper, bins / 'agent-work-recorder-build.json')
        package_info.verify(bins, skill)
        install_manifest = bins / 'agent-work-recorder-build.json'
        installed = json.loads(install_manifest.read_text())
        saved = install_manifest.read_text()
        installed['rust_source'] = 'older-source'
        install_manifest.write_text(json.dumps(installed))
        rejected(lambda: package_info.verify(bins, skill), 'does not match current repository source')
        install_manifest.write_text(saved)
        helper.write_bytes(b'different helper bytes')
        rejected(lambda: package_info.verify(bins, skill), 'rec-capture differs')
        helper.write_bytes(b'non-executable helper metadata fixture')
        original = (skill / 'SKILL.md').read_bytes()
        (skill / 'SKILL.md').write_bytes(original + b'\nlocal user edit\n')
        rejected(lambda: package_info.verify(bins, skill), 'Installed skill differs')
        rejected(lambda: package_info.package_skill(root / 'packages'), 'Existing package was changed')
        assert (skill / 'SKILL.md').read_bytes().endswith(b'local user edit\n')
        (skill / 'SKILL.md').write_bytes(original)
        manifest_path = skill / 'BUILD.json'
        original_manifest = manifest_path.read_text()
        metadata['sha256'] = 'wrong'
        manifest_path.write_text(json.dumps(metadata))
        rejected(lambda: package_info.verify(bins, skill), 'BUILD.json does not match')
        manifest_path.write_text(original_manifest)
        metadata = json.loads(original_manifest)
        metadata['revision'] = 'incorrect-revision'
        manifest_path.write_text(json.dumps(metadata))
        rejected(lambda: package_info.verify(bins, skill), 'BUILD.json does not match')
        rejected(lambda: package_info.package_skill(root / 'packages'), 'Existing package was changed')
        manifest_path.write_text(original_manifest)
        old_rec = bins / 'old-rec'
        old_rec.write_text('#!/bin/sh\nprintf "rec 0.1.0\\n"\n'); old_rec.chmod(0o700)
        rejected(lambda: package_info.write_manifest(old_rec, helper, root / 'wrong.json'), 'does not match current source')
        assert not (root / 'wrong.json').exists()
    print('Package identity checks passed: reuse, binary mismatch, skill/metadata edits and old-build rejection.')


if __name__ == '__main__': main()
