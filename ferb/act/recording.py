"""Task-owned, sampled target videos and decoded-frame inspection. Never captures a display or moves focus.

With sound (L26): `with_audio` also captures the target app's sound through Hearing (ferb/act/listen.py) from just
after the first frame. recording_stop attaches the redacted, on-device transcript with segment times on the video's own
timeline, labelled with where the sound came from (outside content, MP2). The video keeps its frames only and the
sound file is deleted once transcribed (owner decision, 2026-10-14). A recording the task's end or a shutdown stops is
not transcribed; its sound is deleted.

Each frame comes from its driver with its secret fields painted over (L13); a frame that couldn't be is withheld
(Withheld) and skipped, and the recording goes on: its result counts the withheld frames and says why, and repeats
any note a driver gave about how frames were masked (a browser page masked from Accessibility alone).

Frames are staged in FERB's own folder (the daemon's ~/.ferb/run/recordings), never beside the output. The output
folder is checked against the protection lists as macOS opened it and kept open for the recording's life: the receipt,
the video and the final receipt are written only through that open folder, never following or replacing a link that
appears there later (rule 10)."""
import asyncio
from dataclasses import dataclass, field
from datetime import datetime, timezone
import errno
import json
import math
import os
from pathlib import Path
import shutil
import tempfile
import time
from typing import Any, Awaitable, Callable
from uuid import uuid4

from pydantic import BaseModel, Field

from ferb.act.fs.files import PathRefused, destination_closed, make_folders, opened_path, resolve
from ferb.core.errors import Refused, Withheld
from ferb.core.types import ActionSpec, FileAccess, Identity, RiskLevel, Target, ToolContext, ToolResult

Capture = Callable[[], Awaitable[bytes | tuple[bytes, str]]]   # a frame, or a frame and a note on its masking


def _frame(got: bytes | tuple[bytes, str]) -> tuple[bytes, str]:
    return (got, "") if isinstance(got, (bytes, bytearray)) else (bytes(got[0]), str(got[1]))


def executable(name: str) -> str:
    path = shutil.which(name)
    if not path:                          # launchd's PATH omits the normal user/Homebrew install locations
        path = next((str(p/name) for p in (Path('/opt/homebrew/bin'), Path('/usr/local/bin'), Path.home()/'.local/bin')
                     if (p/name).is_file() and os.access(p/name, os.X_OK)), None)
    if not path:
        raise Refused(f'{name} is not installed/on PATH; target recording needs ffmpeg and ffprobe')
    return path


async def command(*args: str) -> bytes:
    proc = await asyncio.create_subprocess_exec(*args, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
    try:
        out, err = await proc.communicate()
    except BaseException:
        if proc.returncode is None:
            proc.kill()
        await proc.wait()
        raise
    if proc.returncode:
        raise Refused(f'{Path(args[0]).name} failed: {err.decode(errors="replace")}')
    return out


@dataclass
class _Recording:
    id: str
    owner: str
    target: dict[str, Any]
    path: Path
    folder: Path
    capture: Capture
    fps: float
    started: float
    at: str = field(default_factory=lambda: datetime.now(timezone.utc).isoformat())
    times: list[float] = field(default_factory=list)
    error: str | None = None
    withheld: int = 0                    # frames skipped because their secret fields couldn't be painted over (L13)
    withheld_reason: str | None = None
    masking: set[str] = field(default_factory=set)     # the drivers' notes on how frames were masked
    ended: float | None = None
    loop: asyncio.Task[None] | None = None
    halt: asyncio.Event = field(default_factory=asyncio.Event)
    lock: asyncio.Lock = field(default_factory=asyncio.Lock)
    result: dict[str, Any] | None = None
    listening: Any = None                    # the sound being captured with it (with_audio), else None
    audio_at: float = 0.0                    # when that sound starts, in seconds of the video
    language: str | None = None
    sources: tuple[tuple[str, str], ...] = ()   # where the sound came from: ('audio', bundle), a tab's page
    out: int = -1                  # the output folder as checked, open until the recording is finished

    @property
    def receipt(self) -> Path:
        return self.path.with_suffix(self.path.suffix + '.json')


_NEW = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC     # a new file, never through a link


def _publish(video: Path, out: int, name: str) -> None:
    """The video into the checked output folder: a hard link (atomic), or an exclusive copy across volumes; never
    replacing or following anything already at `name`."""
    try:
        os.link(video, name, dst_dir_fd=out)
        return
    except OSError as e:
        if e.errno != errno.EXDEV:
            raise
    fd = os.open(name, _NEW, 0o644, dir_fd=out)
    try:
        with os.fdopen(fd, 'wb') as dst, open(video, 'rb') as src:     # the new file closes even if the video won't open
            shutil.copyfileobj(src, dst)
    except BaseException:
        os.unlink(name, dir_fd=out)
        raise


def _replace_receipt(out: int, name: str, text: str) -> None:
    """The finished receipt, written to a new temporary file in the checked folder and renamed over `name`: whatever
    stands there now (even a link) is replaced, never written through."""
    temp = f'.{name}.{uuid4().hex[:8]}.ferb-tmp'
    fd = os.open(temp, _NEW, 0o644, dir_fd=out)
    try:
        with os.fdopen(fd, 'w') as stream:
            stream.write(text)
        os.replace(temp, name, src_dir_fd=out, dst_dir_fd=out)
    except BaseException:
        try:
            os.unlink(temp, dir_fd=out)
        except FileNotFoundError:
            pass
        raise


class RecordingManager:
    def __init__(self, now: Callable[[], float] = time.monotonic, *, sanitize: Callable[[bytes], bytes] | None = None,
                 hearing: Any = None, staging: Path | None = None) -> None:
        """`hearing`: ferb/act/listen.py's Hearing, for recordings with sound. `staging`: where frames are staged, a
        folder only FERB writes (the daemon passes ~/.ferb/run/recordings); None: a private temporary folder."""
        from ferb.sanitize.images import ImageRedactor
        from ferb.sanitize.redact import Redactor
        self.sanitize = sanitize or ImageRedactor(Redactor())
        self.now = now
        self.hearing = hearing                   # ferb/act/listen.py's Hearing: recordings with sound
        self.records: dict[str, _Recording] = {}
        self._starts = asyncio.Lock()
        self.staging = staging

    def _staging(self) -> Path:
        if self.staging is None:
            self.staging = Path(tempfile.mkdtemp(prefix='ferb-recordings-'))
        self.staging.mkdir(parents=True, exist_ok=True, mode=0o700)
        return self.staging

    def sweep(self) -> None:
        """At daemon start: no recording survives a restart, so the staging folders the last run left behind go (only
        FERB's own `rec-*` folders, in its own staging folder; never through a link)."""
        if self.staging is None or self.records:
            return
        try:
            entries = list(os.scandir(self.staging))
        except (FileNotFoundError, NotADirectoryError):
            return
        failed: OSError | None = None
        for entry in entries:
            if entry.name.startswith('rec-') and entry.is_dir(follow_symlinks=False):
                try:
                    shutil.rmtree(entry.path)
                except OSError as e:                       # the others still go; the daemon journals the first
                    failed = failed or e
        if failed is not None:
            raise failed

    def _get(self, rid: str, owner: str) -> _Recording:
        rec = self.records.get(rid)
        if rec is None:
            raise Refused('unknown recording id')
        if rec.owner != owner:
            raise Refused('recording belongs to another task')
        return rec

    def _info(self, r: _Recording) -> dict[str, Any]:
        path, receipt = r.path, r.receipt
        if r.out >= 0:                    # the checked folder, wherever it is now (it may have been moved since)
            try:
                where = opened_path(r.out)
                path, receipt = where / r.path.name, where / r.receipt.name
            except OSError:
                pass
        return {'id': r.id, 'resource': 'recording', 'target': r.target, 'path': str(path),
                'receipt': str(receipt), 'started_at': r.at, 'requested_fps': r.fps,
                'frames': len(r.times), 'first_frame_s': r.times[0] if r.times else None,
                'last_frame_s': r.times[-1] if r.times else None,
                'duration_s': max(0, (r.ended if r.ended is not None else self.now()) - r.started),
                'capture_complete': False, 'content_reviewed': False, 'error': r.error,
                'frames_withheld': r.withheld, 'withheld_reason': r.withheld_reason, 'masking': sorted(r.masking),
                'running': not r.halt.is_set() and r.result is None}

    async def start(self, owner: str, target: dict[str, Any], path: Path, capture: Capture,
                    fps: float = 2, audio: tuple[str, int] | None = None, language: str | None = None,
                    sources: tuple[tuple[str, str], ...] = (), *,
                    closed: Callable[[Path], str | None] | None = None) -> dict[str, Any]:
        """`audio`: (bundle id, pid) of the app whose sound is recorded with the frames (with_audio); `sources`: the
        page it plays in, besides that app (its read sources once the transcript is attached). `closed`: the
        protection lists for a file written at a path (rule 10), checked through the folders as macOS opens them,
        before and while the output's folders are made."""
        async with self._starts:
            executable('ffmpeg'); executable('ffprobe')
            if not math.isfinite(fps) or not 0.2 <= fps <= 10:
                raise Refused('sample rate must be between 0.2 and 10 frames/second')
            path = path.resolve()
            if path.suffix.lower() != '.mp4':
                raise Refused('recording output must be an .mp4 path')
            if path.exists() or path.with_suffix(path.suffix + '.json').exists() or any(
                    r.path == path and r.result is None for r in self.records.values()):
                raise Refused('recording path or receipt already exists; choose a new path')
            if any(r.target == target and r.result is None for r in self.records.values()):
                raise Refused('this target already has an active recording')
            refusal = destination_closed(path, closed)
            if refusal:
                raise Refused(refusal)
            # Capture before allocating files; no video should exist if the target cannot be reached.
            raw, note = _frame(await capture())
            first = await asyncio.to_thread(self.sanitize, raw)
            began = self.now()
            if audio is not None and self.hearing is None:
                raise Refused('FERB has no way to hear sound here; record without with_audio')
            try:
                make_folders(path.parent, closed)          # each folder made inside one FERB opened and checked
            except PathRefused as e:
                raise Refused(str(e)) from None
            out = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)   # kept for the recording's life
            folder: Path | None = None
            receipt = False
            listening = None
            receipt_fd = -1
            try:
                if closed is not None:                     # the folder it writes into, as macOS opened it
                    where = opened_path(out)
                    refusal = closed(where / path.name) or closed(where / (path.name + '.json'))
                    if refusal:
                        raise Refused(refusal)
                try:
                    os.stat(path.name, dir_fd=out, follow_symlinks=False)
                    raise Refused('recording path or receipt already exists; choose a new path')
                except FileNotFoundError:
                    pass
                try:
                    receipt_fd = os.open(path.name + '.json', _NEW, 0o644, dir_fd=out)
                except FileExistsError:
                    raise Refused('recording path or receipt already exists; choose a new path') from None
                receipt = True
                folder = Path(tempfile.mkdtemp(prefix='rec-', dir=self._staging()))
                listening = await self.hearing.start(owner, *audio) if audio is not None else None
                audio_at = max(0.0, self.now() - began - self.hearing.lag(listening)) if listening is not None else 0.0
                r = _Recording('rec-' + uuid4().hex[:12], owner, dict(target), path, folder, capture, fps, began,
                               out=out, listening=listening, audio_at=audio_at, language=language,
                               sources=tuple(sources) + (('audio', audio[0]),) if audio is not None else ())
                r.masking.update({note} - {''})
                (folder/'00000000.jpg').write_bytes(first)
                r.times.append(0.0)
                stream = os.fdopen(receipt_fd, 'w')
                receipt_fd = -1                            # the stream owns it now
                with stream:
                    json.dump(self._info(r) | {'sample_times_s': r.times}, stream, indent=2)
            except BaseException:
                if receipt_fd >= 0:
                    os.close(receipt_fd)
                if folder is not None:
                    shutil.rmtree(folder, ignore_errors=True)
                if receipt:
                    try:
                        os.unlink(path.name + '.json', dir_fd=out)
                    except FileNotFoundError:
                        pass
                os.close(out)
                if listening is not None:
                    await self.hearing.discard(listening)
                raise
            self.records[r.id] = r
            r.loop = asyncio.create_task(self._loop(r))
            return self._info(r) | {'next': 'Stop with recording_stop, then inspect decoded video frames before claiming coverage.'}

    async def sample(self, rid: str, owner: str) -> None:
        r = self._get(rid, owner)
        async with r.lock:
            if r.halt.is_set():
                return
            try:
                raw, note = _frame(await r.capture())
                image = await asyncio.to_thread(self.sanitize, raw)
                if r.halt.is_set():
                    return
                at = self.now() - r.started
                (r.folder/f'{len(r.times):08d}.jpg').write_bytes(image)
                r.times.append(at)
                r.masking.update({note} - {''})
            except Withheld as exc:              # this frame only: the recording goes on
                r.withheld += 1
                r.withheld_reason = str(exc)
            except Exception as exc:
                r.error = f'{type(exc).__name__}: {exc}'
                r.ended = self.now()
                r.halt.set()

    async def _loop(self, r: _Recording) -> None:
        while not r.halt.is_set():
            try:
                await asyncio.wait_for(r.halt.wait(), 1/r.fps)
            except asyncio.TimeoutError:
                await self.sample(r.id, r.owner)

    async def stop(self, rid: str, owner: str, reason: str | None = None, transcribe: bool = True) -> dict[str, Any]:
        """`transcribe=False` (the task ended, shutdown): the sound is deleted untranscribed."""
        r = self._get(rid, owner)
        r.halt.set()
        if r.loop and r.loop is not asyncio.current_task():
            r.loop.cancel()
            await asyncio.gather(r.loop, return_exceptions=True)
        async with r.lock:
            if r.result is not None:
                return r.result
            r.ended = r.ended if r.ended is not None else self.now()
            r.error = r.error or reason
            try:
                heard = await self.hearing.finish(r.listening) if r.listening is not None else None
            except asyncio.CancelledError:          # its stop goes on: the sound is deleted once it has finished
                self.hearing.discard_later(r.listening)
                raise
            try:
                result = self._info(r) | {'running': False, 'released': True}
                try:
                    # Timestamped concat preserves slow captures and variable intervals instead of speeding up time.
                    end = max(r.ended-r.started, r.times[-1]+0.04)
                    timeline = ['ffconcat version 1.0']
                    for i, at in enumerate(r.times):
                        following = r.times[i+1] if i+1 < len(r.times) else end
                        timeline.extend([f"file '{i:08d}.jpg'", f'duration {max(0.04, following-at):.6f}'])
                    timeline.append(f"file '{len(r.times)-1:08d}.jpg'")
                    (r.folder/'frames.txt').write_text('\n'.join(timeline)+'\n')
                    video = r.folder/'video.mp4'
                    await command(executable('ffmpeg'), '-v', 'error', '-nostdin', '-f', 'concat', '-safe', '1',
                                  '-i', str(r.folder/'frames.txt'), '-vf',
                                  'scale=1280:720:force_original_aspect_ratio=decrease,pad=1280:720:(ow-iw)/2:(oh-ih)/2,'
                                  f'fps=25,tpad=stop_mode=clone:stop_duration={end}',
                                  '-c:v', 'libx264', '-preset', 'veryfast', '-pix_fmt', 'yuv420p', '-fps_mode', 'cfr', '-r', '25',
                                  '-t', str(end), '-movflags', '+faststart', str(video))
                    _publish(video, r.out, r.path.name)    # through the checked folder; never replaces anything
                    result.update(capture_complete=r.error is None and not (heard or {}).get('error'),
                                  bytes=os.stat(r.path.name, dir_fd=r.out, follow_symlinks=False).st_size)
                except Exception as exc:
                    result.update(error=f'{r.error + "; " if r.error else ""}{type(exc).__name__}: {exc}', capture_complete=False)
                finally:
                    shutil.rmtree(r.folder, ignore_errors=True)
                if heard is not None:
                    result.update(await self._sound(r, heard) if transcribe else {
                        'audio_track': False, 'audio_note': 'not transcribed: the recording ended with its task, '
                                                            'and its sound was deleted'})
                r.result = result
                try:
                    _replace_receipt(r.out, r.receipt.name, json.dumps(result | {'sample_times_s': r.times}, indent=2))
                except OSError as exc:
                    result.update(error=f'recording receipt could not be finalized: {exc}', capture_complete=False)
                finally:
                    os.close(r.out)
                    r.out = -1
                return result
            finally:                                # owner decision: the sound goes once no longer needed,
                if r.listening is not None:         # however stop() ends (a cancelled encode included)
                    r.listening.path.unlink(missing_ok=True)

    async def _sound(self, r: _Recording, heard: dict[str, Any]) -> dict[str, Any]:
        """What the sound added: its file and facts, and the redacted transcript on the video's timeline."""
        out: dict[str, Any] = {'audio_track': False, 'audio_s': round(float(heard.get('audio_s') or 0), 2),
                               'audio_peak': heard.get('peak', 0.0), 'audio_offset_s': round(r.audio_at, 3)}
        if heard.get('error'):
            out['audio_error'] = self.hearing.redact(str(heard['error']))
        try:
            out.update(await self.hearing.transcript(r.listening.path, heard, r.language, offset=r.audio_at))
        except Refused as e:
            out['transcript_error'] = str(e)
        return out

    async def end_task(self, owner: str) -> list[dict[str, Any]]:
        owned = [r for r in self.records.values() if r.owner == owner]
        results = [await self.stop(r.id, owner, 'task ended before recording was explicitly stopped', transcribe=False)
                   for r in owned if r.result is None]
        for r in owned:
            self.records.pop(r.id, None)
        return results

    async def shutdown(self) -> None:
        await asyncio.gather(*(self.end_task(owner) for owner in {r.owner for r in self.records.values()}))


async def inspect_video(path: Path, times: list[float] | None = None) -> tuple[dict[str, Any], bytes]:
    """Read real encoded pixels. This returns evidence for a model/human, never a semantic 'verified' verdict."""
    if not path.is_file():
        raise Refused('video file does not exist')
    raw = await command(executable('ffprobe'), '-v', 'error', '-protocol_whitelist', 'file,pipe',
                        '-show_entries', 'format=duration,size', '-of', 'json', str(path))
    meta = json.loads(raw)['format']
    duration = float(meta['duration'])
    if not math.isfinite(duration) or duration <= 0:
        raise Refused('video has no finite positive duration')
    samples = times if times is not None else [0.0, duration/2, max(0.0, duration-0.15)]
    if not 1 <= len(samples) <= 6 or any(not math.isfinite(t) or t < 0 or t >= duration for t in samples):
        raise Refused('request 1–6 frame times inside the video duration')
    frames = []
    for t in samples:
        frame = await command(executable('ffmpeg'), '-v', 'error', '-nostdin', '-protocol_whitelist', 'file,pipe',
                              '-ss', str(t), '-i', str(path), '-frames:v', '1', '-vf',
                              'scale=640:360:force_original_aspect_ratio=decrease,pad=640:360:(ow-iw)/2:(oh-ih)/2',
                              '-f', 'image2pipe', '-c:v', 'mjpeg', 'pipe:1')
        if not frame:
            raise Refused(f'could not decode the frame at {t}s')
        frames.append(frame)
    image, width, height = await asyncio.to_thread(_review_sheet, frames, samples)
    return {'path': str(path), 'duration_s': duration, 'bytes': int(meta['size']), 'sample_times_s': samples,
            'review_width': width, 'review_height': height,
            'next': 'Compare these decoded frames with the requested targets/actions; choose other times if coverage is unclear.'}, image


def _review_sheet(frames: list[bytes], times: list[float]) -> tuple[bytes, int, int]:
    import Quartz as Q
    from ferb.act.mac.screenshot import _jpeg, _draw_marks
    width, height = min(2,len(frames))*640, math.ceil(len(frames)/2)*360
    ctx = Q.CGBitmapContextCreate(None,width,height,8,0,Q.CGColorSpaceCreateDeviceRGB(),Q.kCGImageAlphaPremultipliedLast)
    marks = []
    for i, data in enumerate(frames):
        source = Q.CGImageSourceCreateWithData(data,None)
        image = Q.CGImageSourceCreateImageAtIndex(source,0,None)
        x, y = (i%2)*640, (i//2)*360
        Q.CGContextDrawImage(ctx,Q.CGRectMake(x,height-y-360,640,360),image)
        marks.append((f'{times[i]:.2f}s',(x,y,640,360)))
    return _jpeg(_draw_marks(Q.CGBitmapContextCreateImage(ctx),marks)),width,height


class RecordingStop:
    name = 'recording.stop'
    description = 'Stop and finalize a task-owned target video. Reports capture gaps/errors; inspect the saved pixels before claiming the requested process was recorded.'
    identities = {Identity.OWNER, Identity.FERB}
    scopes, risk, reversible = {'fs:write'}, RiskLevel.LOW, True
    # S1: it only publishes into the new path mac.record/browser.record reserved (start refuses an existing one)
    creates_new_only = True
    class Input(BaseModel):
        id: str
    def __init__(self, manager: RecordingManager) -> None: self.manager = manager
    def file_paths(self, args: Any) -> list[tuple[str, FileAccess]]:
        rec = self.manager.records.get(args.id)        # the video it publishes, reserved at start (rule 10)
        return [(str(rec.path), "write")] if rec is not None else []
    def locks(self, args: Any) -> list[str]: return ['recording:'+args.id]
    def describe(self, args: Any) -> ActionSpec: return ActionSpec('write_file')
    async def run(self, args: Any, ctx: ToolContext) -> ToolResult:
        rec = self.manager.records.get(args.id)
        result = await self.manager.stop(args.id, ctx.task_id)
        # a transcript of what the target played is outside content (MP2): labelled with where the sound came from
        sources = rec.sources if rec is not None and rec.listening is not None else ()
        return ToolResult(result, is_error=not result['capture_complete'], read_sources=sources)


def recording_id(video: Path) -> str:
    """The FERB recording a video came from (named in its receipt), else the video itself."""
    try:
        rid = json.loads(video.with_suffix(video.suffix + '.json').read_text()).get('id')
    except (OSError, ValueError, AttributeError):
        rid = None
    return str(rid) if rid else str(video)


class RecordingInspect:
    name = 'recording.inspect'
    description = 'Decode a saved video into a labelled contact sheet (default: beginning/middle/end). Supply times_s around important actions. Check actual target/content visually, not just file metadata. Does not change any app/window.'
    identities = {Identity.OWNER, Identity.FERB}
    scopes, risk, reversible = {'fs:read'}, RiskLevel.LOW, True
    observation = True                                 # only reads: the final checker may use it (V3, W4)
    target_args = ("path",)                            # Target.path, normalized by the gateway (P19 hash)
    class Input(BaseModel):
        path: str
        times_s: list[float] | None = Field(None, min_length=1, max_length=6)
    def locks(self, args: Any) -> list[str]: return []
    def file_paths(self, args: Any) -> list[tuple[str, FileAccess]]: return [(args.path, "read")]   # rule 10
    def describe(self, args: Any) -> ActionSpec: return ActionSpec('read',Target(path=args.path))
    async def run(self, args: Any, ctx: ToolContext) -> ToolResult:
        path = resolve(args.path,ctx,must_exist=True)
        info,image = await inspect_video(path,args.times_s)
        # recorded screen content can show outside pages and messages: label it as a recording, not a local file
        return ToolResult(info,image=image,read_sources=(('recording',recording_id(path)),))
