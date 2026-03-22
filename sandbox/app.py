"""
Varg Agent - Sandbox Sidecar
Provides code execution in disposable Docker containers, git cloning, and file reading.
Called by the Varg agent via HTTP on the internal Docker network.
"""

from flask import Flask, request, jsonify
import subprocess
import uuid
import os
import shutil

app = Flask(__name__)

MAX_TIMEOUT = 60
DEFAULT_TIMEOUT = 30
MAX_OUTPUT_BYTES = 500_000  # 500KB cap
REPOS_DIR = "/tmp/repos"

os.makedirs(REPOS_DIR, exist_ok=True)


@app.route('/health', methods=['GET'])
def health():
    return jsonify({"status": "ok"})


@app.route('/run', methods=['POST'])
def run_container():
    """Run a command in a disposable Docker container."""
    data = request.json or {}
    image = data.get('image', '')
    command = data.get('command', '')
    timeout = min(max(int(data.get('timeout', DEFAULT_TIMEOUT)), 1), MAX_TIMEOUT)
    # Optional: mount a cloned repo into the container
    repo_id = data.get('repo_id', '')

    if not image or not command:
        return jsonify({'error': 'image and command are required'}), 400

    container_name = f"sandbox-{uuid.uuid4().hex[:12]}"

    docker_cmd = [
        'docker', 'run', '--rm',
        '--name', container_name,
        '--network', 'none',
        '--memory', '256m',
        '--cpus', '0.5',
        '--pids-limit', '64',
        '--read-only',
        '--tmpfs', '/tmp:size=64m',
    ]

    # Mount cloned repo if requested
    if repo_id:
        repo_path = os.path.join(REPOS_DIR, repo_id)
        if os.path.isdir(repo_path):
            docker_cmd += ['-v', f'{repo_path}:/work:ro']

    docker_cmd += [image, 'sh', '-c', command]

    timed_out = False
    try:
        result = subprocess.run(
            docker_cmd,
            capture_output=True,
            timeout=timeout
        )
        stdout = result.stdout[:MAX_OUTPUT_BYTES].decode('utf-8', errors='replace')
        stderr = result.stderr[:MAX_OUTPUT_BYTES].decode('utf-8', errors='replace')
        exit_code = result.returncode
    except subprocess.TimeoutExpired:
        timed_out = True
        stdout, stderr, exit_code = '', 'Execution timed out', -1
        try:
            subprocess.run(['docker', 'kill', container_name],
                           capture_output=True, timeout=5)
        except Exception:
            pass
    except Exception as e:
        return jsonify({'error': str(e)}), 500
    finally:
        try:
            subprocess.run(['docker', 'rm', '-f', container_name],
                           capture_output=True, timeout=5)
        except Exception:
            pass

    return jsonify({
        'stdout': stdout,
        'stderr': stderr,
        'exit_code': exit_code,
        'timed_out': timed_out
    })


@app.route('/clone', methods=['POST'])
def clone_repo():
    """Clone a git repository to a temporary directory."""
    data = request.json or {}
    url = data.get('url', '')
    depth = min(max(int(data.get('depth', 1)), 1), 100)

    if not url.startswith('https://'):
        return jsonify({'error': 'Only HTTPS URLs allowed'}), 400

    repo_id = uuid.uuid4().hex[:12]
    repo_path = os.path.join(REPOS_DIR, repo_id)

    try:
        subprocess.run(
            ['git', 'clone', '--depth', str(depth), '--single-branch', url, repo_path],
            capture_output=True, timeout=120, check=True
        )
    except subprocess.TimeoutExpired:
        return jsonify({'error': 'Clone timed out'}), 504
    except subprocess.CalledProcessError as e:
        return jsonify({'error': e.stderr.decode('utf-8', errors='replace')}), 500

    # Collect file listing (skip .git)
    files = []
    for root, dirs, filenames in os.walk(repo_path):
        dirs[:] = [d for d in dirs if d != '.git']
        for f in filenames:
            rel = os.path.relpath(os.path.join(root, f), repo_path)
            files.append(rel)

    return jsonify({
        'path': repo_path,
        'repo_id': repo_id,
        'file_count': len(files),
        'files': files[:500]
    })


@app.route('/read', methods=['POST'])
def read_repo_file():
    """Read a file from a cloned repo."""
    data = request.json or {}
    repo_id = data.get('repo_id', '')
    file_path = data.get('path', '')

    if not repo_id or not file_path:
        return jsonify({'error': 'repo_id and path are required'}), 400

    # Prevent path traversal
    full_path = os.path.normpath(os.path.join(REPOS_DIR, repo_id, file_path))
    if not full_path.startswith(os.path.join(REPOS_DIR, repo_id)):
        return jsonify({'error': 'Invalid path'}), 400

    if not os.path.isfile(full_path):
        return jsonify({'error': 'File not found'}), 404

    try:
        with open(full_path, 'r', errors='replace') as f:
            content = f.read(MAX_OUTPUT_BYTES)
        return jsonify({'content': content, 'path': file_path})
    except Exception as e:
        return jsonify({'error': str(e)}), 500


@app.route('/cleanup', methods=['POST'])
def cleanup_repo():
    """Remove a cloned repo."""
    data = request.json or {}
    repo_id = data.get('repo_id', '')

    if not repo_id:
        return jsonify({'error': 'repo_id is required'}), 400

    repo_path = os.path.join(REPOS_DIR, repo_id)
    if not os.path.isdir(repo_path):
        return jsonify({'error': 'Repo not found'}), 404

    shutil.rmtree(repo_path, ignore_errors=True)
    return jsonify({'status': 'removed', 'repo_id': repo_id})


if __name__ == '__main__':
    print("[sandbox] Starting sandbox sidecar on :5001")
    app.run(host='0.0.0.0', port=5001)
