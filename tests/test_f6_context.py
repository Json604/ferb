"""P3/P27/P31 protected context and owner-facing reporting are separate from evidence reports."""
from ferb.core.context import ContextBlock, assemble_context
from ferb.orchestrator.language import internal_terms


def test_context_drops_activity_before_memory_and_keeps_complete_owner_request(monkeypatch):
    monkeypatch.setattr("ferb.core.context.count_tokens", len)
    blocks = [ContextBlock('identity', 'identity', protected=True),
              ContextBlock('ACTIVITY ' * 100, 'trail.query', priority=1),
              ContextBlock('IMPORTANT MEMORY', 'memory.read', priority=2),
              ContextBlock('YOUR COMPLETE REQUEST', 'request', protected=True),
              ContextBlock('CONFIRMATIONS AND OUTCOMES', 'state', protected=True)]
    result = assemble_context(blocks, budget_tokens=600)
    assert 'YOUR COMPLETE REQUEST' in result and 'CONFIRMATIONS AND OUTCOMES' in result
    assert 'IMPORTANT MEMORY' in result
    assert 'ACTIVITY ' * 100 not in result
    assert 'trail.query' in result


def test_language_audit_does_not_rewrite_the_answer():
    text = 'The gateway could not check the worker runtime.'
    assert internal_terms(text) == ['gateway', 'worker runtime']
    assert text == 'The gateway could not check the worker runtime.'


def test_selected_procedure_and_owner_answer_are_checked_in_same_task(daemon):
    import json
    from ferb.memory.learning import ProcedurePart
    from ferb.memory.lineage import PROCEDURE_LABEL
    from tests.conftest import chat, install, outcome, sup, task
    spec = task('Inspect the session file')
    spec['procedure_id'] = 'procedure-session'
    inspected = []
    supervisor = daemon.daemon.supervisor
    supervisor.procedure_problem = lambda key: None           # a current procedure (I5 checks it before the task)
    supervisor.procedure_details = lambda key, worker, slots: inspected.append((key, worker.id, slots)) or [
        ProcedurePart(PROCEDURE_LABEL, 'Read and verify the file.', True, frozenset({'ferb'}), (key,))]
    supervisor.on_episode = lambda key: inspected.append(('episode', key))
    supervisor.procedure_outcome = lambda key, task_id, slots: inspected.append(('outcome', key, task_id, slots))
    install(daemon, [sup('task', 'Checking your file.', spec)], {'worker': json.dumps({
        'status': 'done', 'summary': 'Technical file inspection evidence.', 'answer': 'Your session file is complete.',
        'artifacts': [], 'evidence': ['File inspected'], 'input_notes': []})})
    result = outcome(chat(daemon, 'Inspect the session file'))
    assert result['answer'] == 'Your session file is complete.' and result['verified']   # the flag, no prefix (L11)
    assert any(row[0] == 'procedure-session' for row in inspected)
    assert any(row[0] == 'episode' for row in inspected)
    assert ('outcome', 'procedure-session', result['task_id'], None) in inspected    # no slot values given (L29)


def test_ranked_memory_budget_keeps_best_item_when_all_candidates_do_not_fit(monkeypatch):
    from ferb.orchestrator.context_layers import ranked_memory_blocks
    monkeypatch.setattr('ferb.core.context.count_tokens', len)
    blocks = ranked_memory_blocks([
        {'text': 'BEST ' * 12, 'reference': 'memory.learned id=best', 'origin': 'private'},
        {'text': 'LOWEST ' * 100, 'reference': 'memory.learned id=low', 'origin': 'external:web:example.com'},
    ])
    result = assemble_context([ContextBlock('owner request', 'request', protected=True)] +
                              [block for block, _ in blocks], budget_tokens=600)
    assert 'BEST ' * 12 in result and 'LOWEST ' * 100 not in result
    assert 'memory.learned id=low' in result
    assert blocks[1][1] == {'external:web:example.com'}


# L29 (spec 5): the worker starts from the procedure's filled steps, its intent's lessons and the latest runs ----------

FOOD = "cheapest food order"


def _parts(daemon, proc, slots):
    from tests.support.learning_fixtures import on_loop
    return on_loop(daemon, daemon.daemon.learning.procedure_parts, proc, {}, slots)


def _card(daemon, task_id):
    from tests.support.learning_fixtures import on_loop
    return on_loop(daemon, daemon.daemon.learning.current_card, task_id)


def test_the_worker_gets_the_filled_steps_and_the_latest_runs(daemon) -> None:
    """First the procedure: its steps as a tool plan with this run's slot values filled in. Then one part per verified
    run of its intent, newest first (by when each task ended), with its tool calls and what came back; each cites its
    card, carries the card's origins and is read in full by the card's id. A run that failed, or of another intent, is
    not shown."""
    from tests.support.learning_fixtures import intent_procedure, on_loop, store_intent_run
    proc = intent_procedure(daemon, name=FOOD, slots={"dish": "string"})
    on_loop(daemon, store_intent_run, daemon.daemon.learning, name=FOOD, slots={"dish": "string"}, task_id="food3",
            dish="rajma", ended_at="2026-10-21T09:05:00+00:00", verified=False)
    on_loop(daemon, store_intent_run, daemon.daemon.learning, name="book a table", slots={"dish": "string"},
            task_id="table0", dish="thali", ended_at="2026-10-21T09:06:00+00:00")
    parts = _parts(daemon, proc, {"dish": "paneer"})
    first = parts[0]
    assert first.protected and "browser.type" in first.text and '"paneer"' in first.text and "{dish}" not in first.text
    assert first.refs[0] == proc
    runs = [p for p in parts[1:] if not p.protected]
    cards = [_card(daemon, f"food{n}") for n in (2, 1, 0)]
    assert [r.refs for r in runs] == [(c["id"],) for c in cards]                    # newest first; food3 failed
    assert "food2" in runs[0].text and all('browser.type {"text":' in r.text and '{"ok":true}' in r.text for r in runs)
    assert runs[0].reference == "memory.learned item_id=" + cards[0]["id"]
    assert runs[0].origins == frozenset(cards[0]["origins"])


def test_wrong_or_missing_slot_values_leave_the_template(daemon) -> None:
    """Review Focus 5: without values for its slots (none given, or values the planner gave of the wrong type, which
    the task's start leaves out), each step keeps its {slot} template and nothing raises."""
    from tests.support.learning_fixtures import intent_procedure, worker_context_for
    from ferb.memory.lineage import PROCEDURE_LABEL
    proc = intent_procedure(daemon, name=FOOD, slots={"dish": "string"})
    assert "{dish}" in _parts(daemon, proc, None)[0].text
    assert "{dish}" in _parts(daemon, proc, {})[0].text
    ctx = worker_context_for(daemon, proc, [{"name": "dish", "value": "3"}])           # a number, not a string
    procedure = ctx.split(PROCEDURE_LABEL + ":\n", 1)[1].split("\n\n", 1)[0]
    assert 'browser.type {"text":"{dish}"}' in procedure


def test_runs_are_wrapped_as_untrusted_data(daemon) -> None:
    """The worker's context shows each run inside the untrusted-data wrapping, with the procedure block recorded as
    the selected procedure's source (the card capture finds it by that label)."""
    import json
    import re
    from ferb.memory.lineage import PROCEDURE_LABEL
    from tests.support.learning_fixtures import intent_procedure, on_loop, worker_context_for
    proc = intent_procedure(daemon, name=FOOD, slots={"dish": "string"})
    ctx = worker_context_for(daemon, proc, [{"name": "dish", "value": '"paneer"'}])
    assert "<untrusted-data" in ctx and "food2" in ctx
    wrapped = re.findall(r'<untrusted-data id="(\w+)">\n(.*?)\n</untrusted-data id="\1">', ctx, re.S)
    assert any("food2" in body and 'browser.type {"text":"dal"}' in body for _, body in wrapped)
    procedure = ctx.split(PROCEDURE_LABEL + ":\n", 1)[1].split("\n\n", 1)[0]
    assert 'browser.type {"text":"paneer"}' in procedure
    sources = on_loop(daemon, lambda: [json.loads(r[0]) for r in daemon.daemon.db.conn.execute(
        "SELECT input_json FROM spans WHERE kind='context.source'")])
    assert any(s["label"] == PROCEDURE_LABEL and s["source_refs"][0] == proc for s in sources)


def test_the_worker_gets_its_intents_lessons_only(daemon) -> None:
    """The lessons built on its intent's runs (through an owner's edit of one too) come with the procedure, always
    shown; a lesson of another kind of task does not."""
    from tests.support.learning_fixtures import intent_procedure, on_loop, store_intent_run
    proc = intent_procedure(daemon, name=FOOD, slots={"dish": "string"})
    learning = daemon.daemon.learning

    def lessons() -> None:
        failed = store_intent_run(learning, name=FOOD, slots={"dish": "string"}, task_id="food3", dish="rajma",
                                  ended_at="2026-10-21T09:05:00+00:00", verified=False)
        ok = learning.current_card("food0")
        lesson = learning.put("lesson", "search", {"scope": {"app": "", "site": "menu.example", "intent": FOOD},
                                                   "trigger": "the search box is hidden", "text": "open the menu first",
                                                   "failure": failed["id"], "success": ok["id"]},
                              [failed["id"], ok["id"]], made_by="model:test")
        learning.edit(lesson["id"], "open the menu, then search")
        other = store_intent_run(learning, name="book a table", slots={"dish": "string"}, task_id="table0",
                                 dish="thali", ended_at="2026-10-21T09:06:00+00:00")
        learning.put("lesson", "tables", {"scope": {"app": "", "site": "", "intent": "book a table"},
                                          "trigger": "no tables", "text": "try another time"},
                     [other["id"]], made_by="model:test")
    on_loop(daemon, lessons)
    [part] = [p for p in _parts(daemon, proc, None)[1:] if p.protected]
    assert "the search box is hidden: open the menu, then search" in part.text and "try another time" not in part.text


def test_the_runs_fold_to_fit_a_small_worker_window(daemon) -> None:
    """Spec 5: the runs are fitted to the worker's window. They are optional blocks, newest first: in a small window
    the newest stay whole and each older one is a marker naming its card; in a smaller one, where even their markers
    can't all fit, the oldest fold into one counted note naming their route. The turn is never refused as too large."""
    from ferb.core.types import ModelLimits
    from tests.support.learning_fixtures import intent_procedure, on_loop, store_intent_run, worker_context_for
    proc = intent_procedure(daemon, name=FOOD, slots={"dish": "string"})
    slots = [{"name": "dish", "value": '"paneer"'}]
    daemon.daemon.supervisor.relevant_context = lambda words: ('TODAY-ACTIVITY opened the menu page', 'private',
                                                               'opened the menu page', ['trail:t1'])
    full = worker_context_for(daemon, proc, slots)                                # three runs, all shown

    def more() -> None:
        for n in range(40):
            store_intent_run(daemon.daemon.learning, name=FOOD, slots={"dish": "string"}, task_id=f"more{n:02}",
                             dish=f"dish {n:02}", ended_at=f"2026-10-21T10:{n:02}:00+00:00")
    on_loop(daemon, more)
    oldest = "memory.learned item_id=" + _card(daemon, "food0")["id"]

    def context(room: int) -> str:
        return worker_context_for(daemon, proc, slots, worker_limits=ModelLimits(
            "fake-model", 400_000, 100_000, "test double", max_input_chars=len(full) + room))
    ctx = context(8_000)
    shown = [n for n in range(40) if f"Task more{n:02}," in ctx]
    assert shown and shown == list(range(40 - len(shown), 40)) and len(shown) < 40  # the newest shown, in full
    assert "Task food0," not in ctx and f"[Context omitted; fetch full text: {oldest}]" in ctx
    ctx = context(3_000)
    assert "Task food0," not in ctx and f"[Context omitted; fetch full text: {oldest}]" not in ctx
    assert "more not shown or listed one by one; fetch full text: memory.learned kind=episode" in ctx
