import asyncio
import hashlib
import json
from pathlib import Path

import pytest

from backend.app.downloader.ftp_file import (
    FTPDownloader,
    FtpError,
    describe_ftp_error,
    parse_ftp_target,
    redact_ftp_url,
)
from backend.app.downloader.task_manager import TaskManager, resolve_task_type
from backend.app.models import Task, TaskStatus, TaskType
from backend.app.network_proxy import PrivateDestinationError
from backend.app.schemas import TaskCreate, UrlRecognitionRequest
from backend.app.url_recognition import recognize_url
from pydantic import ValidationError


class FakeFTP:
    def __init__(self, files, *, rest_ok=True, fail_login=False, size_ok=True):
        self.files = files
        self.rest_ok = rest_ok
        self.fail_login = fail_login
        self.size_ok = size_ok
        self.closed = False
        self.commands = []

    def login(self, user='', passwd=''):
        if self.fail_login:
            import ftplib
            raise ftplib.error_perm('530 Login incorrect.')
        self.commands.append('USER ' + user)
        return '230'

    def set_pasv(self, val):
        self.commands.append('PASV ' + str(val))

    def voidcmd(self, cmd):
        self.commands.append(cmd)
        return '200'

    def sendcmd(self, cmd):
        self.commands.append(cmd)
        if cmd.startswith('REST'):
            if not self.rest_ok:
                import ftplib
                raise ftplib.error_perm('350 Restart not allowed')
            return '350 Restarting'
        if cmd.startswith('MDTM '):
            return '213 20260101120000'
        return '200'

    def size(self, filename):
        if not self.size_ok:
            import ftplib
            raise ftplib.error_perm('550 SIZE not allowed')
        data = self.files.get(filename)
        if data is None:
            import ftplib
            raise ftplib.error_perm('550 File not found')
        return len(data)

    def retrbinary(self, cmd, callback, blocksize=8192, rest=None):
        self.commands.append(cmd if rest is None else 'REST %s; %s' % (rest, cmd))
        name = cmd.split(' ', 1)[1]
        data = self.files[name]
        start = int(rest or 0)
        chunk = data[start:]
        for index in range(0, len(chunk), blocksize):
            callback(chunk[index:index + blocksize])
        return '226'

    def quit(self):
        self.closed = True
        return '221'

    def close(self):
        self.closed = True


async def _async_noop(*_args, **_kwargs):
    return None


def test_parse_ftp_target_strips_userinfo_and_defaults_ports():
    target = parse_ftp_target('ftp://alice:s3cret@Nas.Local/pub/file.bin')
    assert target.host == 'nas.local'
    assert target.port == 21
    assert target.username == 'alice'
    assert target.password == 's3cret'
    assert target.remote_path == '/pub/file.bin'
    assert target.implicit_tls is False
    assert 's3cret' not in target.resource_key
    implicit = parse_ftp_target('ftps://backup.example.test:990/secret.iso')
    assert implicit.scheme == 'ftps'
    assert implicit.port == 990
    assert implicit.implicit_tls is True
    assert implicit.username == 'anonymous'


def test_parse_ftp_target_rejects_directories_and_non_ftp():
    with pytest.raises(FtpError, match='FTP'):
        parse_ftp_target('ftp://nas.local/pub/')
    with pytest.raises(FtpError, match='ftp://'):
        parse_ftp_target('http://nas.local/file.bin')


def test_redact_ftp_url_never_keeps_password():
    assert redact_ftp_url('ftp://alice:s3cret@nas.local:2121/pub/a.bin') == 'ftp://nas.local:2121/pub/a.bin'
    assert 's3cret' not in describe_ftp_error(FtpError('FTP login failed'))


def test_auto_task_type_and_schema_accept_ftp():
    assert resolve_task_type(TaskType.AUTO, 'ftp://nas.local/a.bin') is TaskType.FTP
    assert resolve_task_type(TaskType.AUTO, 'ftps://nas.local/a.bin') is TaskType.FTP
    assert resolve_task_type(TaskType.HLS, 'ftp://nas.local/a.bin') is TaskType.FTP
    body = TaskCreate(url='ftp://alice:pw@nas.local/pub/a.bin')
    assert body.url.startswith('ftp://')
    secure = TaskCreate(url='ftps://nas.local/a.bin')
    assert secure.url.startswith('ftps://')
    recognized = UrlRecognitionRequest(url='ftp://nas.local/a.bin')
    assert recognized.url.startswith('ftp://')


def test_task_create_rejects_ftp_mirrors_and_unknown_schemes():
    with pytest.raises(ValidationError):
        TaskCreate(url='file:///tmp/a.bin')
    with pytest.raises(ValidationError):
        TaskCreate(url='javascript:alert(1)')
    with pytest.raises(ValidationError, match='FTP'):
        TaskCreate(url='ftp://nas.local/a.bin', mirrors=['https://cdn.example.test/a.bin'])


def test_create_task_keeps_ftp_even_when_method_is_post(monkeypatch):
    async def run():
        manager = TaskManager()
        monkeypatch.setattr(manager, '_save_db', _async_noop)
        monkeypatch.setattr('backend.app.downloader.task_manager.run_db', _async_noop)
        task = await manager.create_task('ftp://nas.local/a.bin', task_type=TaskType.AUTO, request_method='POST', request_body='e30=')
        assert task.task_type is TaskType.FTP
        http = await manager.create_task('https://cdn.example.test/a.bin')
        assert http.task_type is TaskType.HTTP
    asyncio.run(run())


def test_recognize_url_accepts_ftp_without_http_fetch():
    async def run():
        result = await recognize_url('ftp://nas.local/pub/movie.mp4', headers={})
        assert result.kind == 'file'
        assert result.candidates[0].url == 'ftp://nas.local/pub/movie.mp4'
    asyncio.run(run())


def test_ftp_download_writes_file_and_verifies_checksum(tmp_path):
    payload = b'ftp-payload-' + bytes(range(64))
    digest = hashlib.sha256(payload).hexdigest()
    fake = FakeFTP({'/pub/a.bin': payload})
    async def run():
        task = Task(id='ftp-ok', url='ftp://nas.local/pub/a.bin', task_type=TaskType.FTP, expected_checksum='sha256:' + digest, checksum_algorithm='sha256')
        task.engine_state = {'output_dir': str(tmp_path / 'out'), 'temp_dir': str(tmp_path / 'tmp')}
        downloader = FTPDownloader(task, open_client=lambda _target: fake)
        await downloader.run()
        assert task.status is TaskStatus.DONE
        assert Path(task.output_path).read_bytes() == payload
        assert task.checksum_verified is True
        assert fake.closed is True
    asyncio.run(run())


def test_ftp_resume_uses_rest_when_size_and_identity_match(tmp_path):
    payload = b'ABCDEFGHIJKLMNOPQRSTUVWXYZ012345'
    fake = FakeFTP({'/file.bin': payload})
    task = Task(id='ftp-resume', url='ftp://nas.local/file.bin', task_type=TaskType.FTP)
    task.engine_state = {'output_dir': str(tmp_path / 'out'), 'temp_dir': str(tmp_path / 'tmp')}
    work = tmp_path / 'tmp' / '.tasks' / 'ftp-resume'
    work.mkdir(parents=True)
    (work / 'payload.downloading').write_bytes(payload[:10])
    (work / 'ftp-resume.json').write_text(json.dumps({'version': 1, 'resource_key': 'ftp://nas.local:21/file.bin', 'total': len(payload), 'mdtm': '20260101120000', 'offset': 10}), encoding='utf-8')
    async def run():
        downloader = FTPDownloader(task, open_client=lambda _target: fake)
        await downloader.run()
        assert task.status is TaskStatus.DONE
        assert Path(task.output_path).read_bytes() == payload
        assert any(item.startswith('REST 10') for item in fake.commands)
    asyncio.run(run())


def test_ftp_restarts_when_rest_is_unavailable(tmp_path):
    payload = b'0123456789abcdef'
    fake = FakeFTP({'/file.bin': payload}, rest_ok=False)
    task = Task(id='ftp-no-rest', url='ftp://nas.local/file.bin', task_type=TaskType.FTP)
    task.engine_state = {'output_dir': str(tmp_path / 'out'), 'temp_dir': str(tmp_path / 'tmp')}
    work = tmp_path / 'tmp' / '.tasks' / 'ftp-no-rest'
    work.mkdir(parents=True)
    (work / 'payload.downloading').write_bytes(payload[:6])
    async def run():
        downloader = FTPDownloader(task, open_client=lambda _target: fake)
        await downloader.run()
        assert task.status is TaskStatus.DONE
        assert Path(task.output_path).read_bytes() == payload
        assert not any(item.startswith('REST 6') for item in fake.commands)
    asyncio.run(run())


def test_ftp_login_failure_is_actionable(tmp_path):
    fake = FakeFTP({'/a.bin': b'x'}, fail_login=True)
    task = Task(id='ftp-login', url='ftp://alice:pw@nas.local/a.bin', task_type=TaskType.FTP)
    task.engine_state = {'output_dir': str(tmp_path / 'out'), 'temp_dir': str(tmp_path / 'tmp')}
    async def run():
        def open_client(target):
            fake.login(target.username, target.password)
            return fake
        downloader = FTPDownloader(task, open_client=open_client)
        await downloader.run()
        assert task.status is TaskStatus.FAILED
        assert 'FTP' in task.error_message or '530' in task.error_message or 'login' in task.error_message.lower()
        assert 'pw' not in task.error_message
        assert 'pw' not in (task.error_url or '')
    asyncio.run(run())


def test_browser_originated_ftp_to_private_host_is_blocked(tmp_path, monkeypatch):
    async def boom(_url):
        raise PrivateDestinationError('browser private dest blocked')
    monkeypatch.setattr('backend.app.downloader.ftp_file.ensure_public_destination', boom)
    task = Task(id='ftp-lan', url='ftp://192.168.1.8/a.bin', task_type=TaskType.FTP)
    task.engine_state = {'output_dir': str(tmp_path / 'out'), 'temp_dir': str(tmp_path / 'tmp'), 'browser_originated': True}
    async def run():
        downloader = FTPDownloader(task, open_client=lambda _target: FakeFTP({'/a.bin': b'x'}))
        await downloader.run()
        assert task.status is TaskStatus.FAILED
        blob = (task.error_message or '') + (task.error_hint or '')
        assert 'private' in blob.lower() or 'blocked' in blob.lower()
    asyncio.run(run())


def test_browser_handoff_still_rejects_ftp():
    from backend.app.schemas import BrowserHandoffCreate
    with pytest.raises(ValidationError):
        BrowserHandoffCreate(url='ftp://nas.local/a.bin')
