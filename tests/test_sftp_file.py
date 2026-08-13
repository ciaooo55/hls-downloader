import asyncio
import hashlib
from pathlib import Path
from types import SimpleNamespace

import pytest
from pydantic import ValidationError

from backend.app.downloader.sftp_file import (
    SFTPDownloader,
    SftpError,
    parse_sftp_target,
    redact_sftp_url,
)
from backend.app.downloader.task_manager import resolve_task_type
from backend.app.models import Task, TaskStatus, TaskType
from backend.app.network_proxy import PrivateDestinationError
from backend.app.schemas import TaskCreate, UrlRecognitionRequest
from backend.app.url_recognition import recognize_url


class FakeRemoteFile:
    def __init__(self, data: bytes):
        self._data = data
        self._offset = 0

    def seek(self, offset, whence=0):
        if whence == 0:
            self._offset = int(offset)
        return self._offset

    def read(self, size=-1):
        if size is None or size < 0:
            chunk = self._data[self._offset:]
            self._offset = len(self._data)
            return chunk
        chunk = self._data[self._offset:self._offset + size]
        self._offset += len(chunk)
        return chunk

    def close(self):
        return None


class FakeSFTP:
    def __init__(self, files, *, fail_auth=False, missing=False):
        self.files = files
        self.fail_auth = fail_auth
        self.missing = missing
        self.closed = False
        self.opened = []

    def stat(self, path):
        if self.missing or path not in self.files:
            raise FileNotFoundError(path)
        data = self.files[path]
        return SimpleNamespace(st_size=len(data), st_mtime=1700000000)

    def open(self, path, mode='r'):
        self.opened.append((path, mode))
        return FakeRemoteFile(self.files[path])

    def close(self):
        self.closed = True


class FakeSession:
    def __init__(self, client):
        self.client = client
        self.closed = False

    def close(self):
        self.closed = True
        self.client.close()


async def _async_noop(*_args, **_kwargs):
    return None


def test_parse_sftp_target_defaults_port_and_strips_userinfo():
    target = parse_sftp_target('sftp://alice:secret@NAS.Example.test/home/alice/a.bin')
    assert target.host == 'nas.example.test'
    assert target.port == 22
    assert target.username == 'alice'
    assert target.password == 'secret'
    assert target.remote_path == '/home/alice/a.bin'
    assert target.display_url == 'sftp://nas.example.test:22/home/alice/a.bin'


def test_parse_sftp_target_rejects_directories_and_non_sftp():
    with pytest.raises(SftpError):
        parse_sftp_target('ftp://nas.example.test/a.bin')
    with pytest.raises(SftpError):
        parse_sftp_target('sftp://nas.example.test/home/alice/')


def test_redact_sftp_url_never_keeps_password():
    assert 'secret' not in redact_sftp_url('sftp://alice:secret@nas.example.test/a.bin')
    assert redact_sftp_url('sftp://alice:secret@nas.example.test/a.bin').startswith('sftp://nas.example.test')


def test_auto_task_type_and_schema_accept_sftp():
    assert resolve_task_type(TaskType.AUTO, 'sftp://nas.example.test/a.bin') is TaskType.SFTP
    created = TaskCreate(url='sftp://nas.example.test/a.bin')
    assert created.url.startswith('sftp://')
    recognized = UrlRecognitionRequest(url='sftp://nas.example.test/pub/file.bin')
    assert recognized.url.startswith('sftp://')


def test_task_create_rejects_sftp_mirrors():
    with pytest.raises(ValidationError):
        TaskCreate(url='sftp://nas.example.test/a.bin', mirrors=['https://cdn.example.test/a.bin'])


def test_create_task_keeps_sftp_even_when_method_is_post(monkeypatch):
    from backend.app.downloader import task_manager as manager_module
    from backend.app.downloader.task_manager import TaskManager

    async def fake_save(self, task):
        return None

    monkeypatch.setattr(TaskManager, '_save_db', fake_save)
    monkeypatch.setattr(manager_module, 'run_db', _async_noop)

    async def run():
        manager = TaskManager()
        task = await manager.create_task('sftp://nas.example.test/a.bin', request_method='POST', auto_start=False)
        assert task.task_type is TaskType.SFTP

    asyncio.run(run())


def test_recognize_url_accepts_sftp_without_http_fetch():
    result = asyncio.run(recognize_url('sftp://nas.example.test/a.bin', {}))
    assert result.kind == 'file'
    assert result.final_url.startswith('sftp://')


def test_sftp_download_writes_file_and_verifies_checksum(tmp_path):
    payload = b'hello-sftp'
    digest = hashlib.sha256(payload).hexdigest()
    client = FakeSFTP({'/pub/a.bin': payload})
    task = Task(id='sftp-ok', task_type=TaskType.SFTP, url='sftp://nas.example.test/pub/a.bin')
    task.expected_checksum = f'sha256:{digest}'
    task.checksum_algorithm = 'sha256'
    task.engine_state = {'output_dir': str(tmp_path / 'out'), 'temp_dir': str(tmp_path / 'tmp')}
    downloader = SFTPDownloader(task, open_session=lambda _target: FakeSession(client))
    asyncio.run(downloader.run())
    assert task.status is TaskStatus.DONE
    assert Path(task.output_path).read_bytes() == payload
    assert client.closed is True


def test_sftp_resume_uses_seek_when_identity_matches(tmp_path):
    import json
    payload = b"ABCDEFGHIJ"
    work = tmp_path / "tmp" / ".tasks" / "sftp-resume"
    work.mkdir(parents=True)
    (work / "payload.downloading").write_bytes(payload[:4])
    (work / "sftp-resume.json").write_text(json.dumps({
        "version": 1,
        "resource_key": "sftp://nas.example.test:22/pub/a.bin",
        "total": len(payload),
        "mtime": "1700000000",
        "offset": 4,
    }), encoding="utf-8")
    client = FakeSFTP({"/pub/a.bin": payload})
    task = Task(id="sftp-resume", url="sftp://nas.example.test/pub/a.bin", task_type=TaskType.SFTP)
    task.engine_state = {"output_dir": str(tmp_path / "out"), "temp_dir": str(tmp_path / "tmp")}
    downloader = SFTPDownloader(task, open_session=lambda _target: FakeSession(client))
    asyncio.run(downloader.run())
    assert task.status is TaskStatus.DONE
    assert Path(task.output_path).read_bytes() == payload
    assert client.opened and client.opened[0][0] == "/pub/a.bin"


def test_sftp_auth_failure_is_actionable(tmp_path):
    def boom(_target):
        raise SftpError('SFTP 登录失败，请检查用户名、密码或私钥')
    task = Task(id='sftp-auth', url='sftp://alice:bad@nas.example.test/pub/a.bin')
    task.engine_state = {'output_dir': str(tmp_path / 'out'), 'temp_dir': str(tmp_path / 'tmp')}
    downloader = SFTPDownloader(task, open_session=boom)
    asyncio.run(downloader.run())
    assert task.status is TaskStatus.FAILED
    assert '登录失败' in task.error_message


def test_browser_originated_sftp_to_private_host_is_blocked(tmp_path, monkeypatch):
    from backend.app.downloader import sftp_file as sftp_module

    async def blocked(_url):
        raise PrivateDestinationError('browser private dest blocked')

    monkeypatch.setattr(sftp_module, 'ensure_public_destination', blocked)
    task = Task(id='sftp-priv', url='sftp://192.168.1.8/a.bin')
    task.engine_state = {'output_dir': str(tmp_path / 'out'), 'temp_dir': str(tmp_path / 'tmp'), 'browser_originated': True}
    downloader = SFTPDownloader(task, open_session=lambda _target: FakeSession(FakeSFTP({'/a.bin': b'x'})))
    asyncio.run(downloader.run())
    assert task.status is TaskStatus.FAILED
    assert 'private' in task.error_message.lower() or '登录' not in task.error_message


def test_browser_handoff_still_rejects_sftp():
    from backend.app.schemas import BrowserHandoffCreate
    with pytest.raises(ValidationError):
        BrowserHandoffCreate(url='sftp://nas.local/a.bin')
