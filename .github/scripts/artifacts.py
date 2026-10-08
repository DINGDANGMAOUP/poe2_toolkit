#!/usr/bin/env python3
"""Promote the exact packages tested by a successful main-branch CI push run."""
import argparse
import json
from pathlib import Path
import subprocess

from publish import CONFIG, TARGETS


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    repository = CONFIG['repository']
    endpoint = f'repos/{repository}/actions/workflows/ci.yml/runs?head_sha={commit}&event=push&branch=main&status=success&per_page=20'
    runs = json.loads(subprocess.check_output(['gh', 'api', endpoint]))['workflow_runs']
    names = {f'package-{target}' for target in TARGETS}
    for run in runs:
        if (run['head_sha'] != commit or run['head_branch'] != 'main'
                or run['event'] != 'push' or run['conclusion'] != 'success'
                or run['repository']['full_name'] != repository):
            continue
        artifacts = json.loads(subprocess.check_output([
            'gh', 'api', f'repos/{repository}/actions/runs/{run["id"]}/artifacts?per_page=100']))['artifacts']
        available = {a['name'] for a in artifacts if not a['expired']}
        if not names.issubset(available):
            continue
        args.output.mkdir(parents=True, exist_ok=False)
        for name in sorted(names):
            subprocess.run(['gh', 'run', 'download', str(run['id']), '--repo', repository,
                            '--name', name, '--dir', str(args.output)], check=True)
        print(f'Promoted exact CI artifacts: {run["html_url"]} ({commit})')
        return
    raise RuntimeError('No successful main push CI with unexpired artifacts for this commit. '
                       'Wait for CI, or rerun that commit\'s original CI run, then rerun Release.')


if __name__ == '__main__':
    main()
