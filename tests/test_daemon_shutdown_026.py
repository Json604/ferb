"""Shutdown must finish task cancellation while cleanup services and persistence are still available."""

import asyncio

from tests.conftest import RunningDaemon


def test_shutdown_drains_chat_and_scheduled_finalizers_before_teardown(home):
    daemon = RunningDaemon(home, watch_owner_input=False)
    events = []
    jobs = []
    async def setup():
        d = daemon.daemon
        original_recordings = d.recordings.shutdown
        original_sentinel = d.disturbance.shutdown
        async def recordings():
            events.append('recordings_closed')
            await original_recordings()
        async def sentinel():
            events.append('sentinel_closed')
            await original_sentinel()
        d.recordings.shutdown, d.disturbance.shutdown = recordings, sentinel
        async def work(kind):
            try:
                await asyncio.Event().wait()
            finally:
                await asyncio.sleep(0)
                assert 'recordings_closed' not in events and 'sentinel_closed' not in events
                d._journal().append('test.shutdown_finalizer', {'family': kind})
                events.append(kind + '_finalized')
        for kind, tracked in [('chat', d._turns), ('schedule', d._runs)]:
            job = asyncio.create_task(work(kind))
            jobs.append(job)
            tracked.add(job)
        await asyncio.sleep(0)
    async def shutdown_from_tracked_task():
        current = asyncio.current_task()
        daemon.daemon._turns.add(current)
        try:
            await daemon.daemon.shutdown()
        finally:
            daemon.daemon._turns.discard(current)
    async def cleanup():
        for job in jobs:
            if not job.done():
                job.cancel()
        await asyncio.gather(*jobs, return_exceptions=True)
    try:
        daemon.run(setup())
        daemon.run(shutdown_from_tracked_task())
        assert all(job.done() for job in jobs)
        assert events.index('chat_finalized') < events.index('recordings_closed')
        assert events.index('schedule_finalized') < events.index('sentinel_closed')
    finally:
        daemon.run(cleanup())
        daemon.loop.call_soon_threadsafe(daemon.loop.stop)
        daemon.thread.join(5)
