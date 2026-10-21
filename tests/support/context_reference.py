"""`ferb.core.context.assemble` as it was before a fold group's markers also fold to make room for a higher-priority
block (L29 Task 5 fix round 1, 2026-10-21; copied from c1c3b12): the reference the property test compares the current
`assemble` with, byte for byte, on inputs without a fold group. Helpers are read from the module at call time, so a
test's patched `count_tokens` applies to both."""
from typing import Any

import ferb.core.context as ctx


def assemble_before_priority_folds(blocks: list[Any], *, budget_tokens: int, reserved_tokens: int = 0,
                                   max_chars: int | None = None) -> Any:
    """The pre-change `assemble`, verbatim (see the module docstring)."""
    if budget_tokens <= 0 or reserved_tokens < 0 or (max_chars is not None and max_chars <= 0):
        raise ValueError("context budget must be positive and reserve nonnegative")
    active = [block for block in blocks if block.text]
    omission = [f"[Context omitted; fetch full text: {block.reference}]" for block in active]
    history = [index for index, block in enumerate(active) if block.history and not block.protected]
    kept = {index for index, block in enumerate(active) if block.protected}
    separator = ctx.count_tokens("\n\n")
    cost = {index: ctx.count_tokens(active[index].text) for index in range(len(active))
            if index in kept or index in history}
    gap = {index: ctx.count_tokens(omission[index]) for index in range(len(active))
           if index not in kept and index not in history}
    # the report and a history note are small; they are counted into the decision as their longest form
    note_cost = ctx.count_tokens(ctx._note(len(history), "chat.history")) + separator if history else 0
    report_cost = ctx.count_tokens(ctx._report(budget_tokens, reserved_tokens, 10 ** 9, True, len(active))) + separator
    used = (reserved_tokens + report_cost + note_cost + sum(cost[index] + separator for index in kept) +
            sum(gap[index] + separator for index in gap))
    # characters, as the runtime counts them (Unicode code points), each part with its separator; the report and the
    # history note at their longest
    cap = max_chars if max_chars is not None else float("inf")
    note_chars = len(ctx._note(len(history), active[history[0]].reference.split(" ", 1)[0])) + 2 if history else 0
    used_chars = (len(ctx._report(budget_tokens, reserved_tokens, 10 ** 9, True, len(active))) + 2 + note_chars +
                  sum(len(active[index].text) + 2 for index in kept) + sum(len(omission[index]) + 2 for index in gap))
    folded: dict[str, list[int]] = {}        # fold route -> its folded blocks, lowest priority first
    for index in sorted((i for i in gap if active[i].fold), key=lambda i: active[i].priority):
        if used <= budget_tokens and used_chars <= cap:
            break
        route = active[index].fold
        if route not in folded:              # the run's note, at its longest: two of its longest references
            run = [i for i in gap if active[i].fold == route]
            used += ctx.count_tokens(ctx._fold(10 ** 9, "", "", route)) + 2 * max(gap[i] for i in run) + separator
            used_chars += len(ctx._fold(10 ** 9, "", "", route)) + 2 * max(len(active[i].reference) for i in run) + 2
            folded[route] = []
        folded[route].append(index)
        used -= gap.pop(index) + separator
        used_chars -= len(omission[index]) + 2
    first_folded = {min(run) for run in folded.values()}
    in_folds = {index for run in folded.values() for index in run}
    for index in reversed(history):
        chars = len(active[index].text) + 2
        if used + cost[index] + separator > budget_tokens or used_chars + chars > cap:
            break
        kept.add(index)
        used += cost[index] + separator
        used_chars += chars
    for index, block in sorted(((i, active[i]) for i in gap), key=lambda pair: -pair[1].priority):
        extra, extra_chars = ctx.count_tokens(block.text) - gap[index], len(block.text) - len(omission[index])
        if used + extra <= budget_tokens and used_chars + extra_chars <= cap:
            kept.add(index)
            used += extra
            used_chars += extra_chars
    omitted_history = sum(1 for index in history if index not in kept)
    parts: list[str] = []
    repeats: list[bool] = []             # per part: repeats across this role's calls
    noted = False
    for index, block in enumerate(active):
        if index in kept:
            parts.append(block.text)
            repeats.append(block.cache)
        elif index in history:
            if not noted:                # the older entries collapse into one note where the first one stood
                parts.append(ctx._note(omitted_history, block.reference.split(" ", 1)[0]))
                repeats.append(block.cache)
                noted = True
        elif index in in_folds:
            if index in first_folded:    # a run's folded blocks: one note where the first one stood
                run = sorted(folded[block.fold])
                parts.append(ctx._fold(len(run), active[run[0]].reference, active[run[-1]].reference, block.fold))
                repeats.append(False)
        else:
            parts.append(omission[index])
            repeats.append(False)        # an omitted block ends the cacheable prefix
    parts = parts or [""]
    body = "\n\n".join(parts)
    content = ctx.count_tokens(body)
    over = content + reserved_tokens > budget_tokens
    report = ctx._report(budget_tokens, reserved_tokens, content, over, len(active) - len(kept))
    # Header/omission references also consume context. Report their unavoidable overhead truthfully.
    if not over and content + ctx.count_tokens(report) + separator + reserved_tokens > budget_tokens:
        report = ctx._report(budget_tokens, reserved_tokens, content, True, len(active) - len(kept))
    parts.append(report)
    chars = len("\n\n".join(parts))
    if max_chars is not None and chars > max_chars:     # protected text alone is over the cap: refused, never sent
        raise ctx.InputTooLarge(chars, max_chars)
    cacheable = 0
    for repeat in repeats:
        if not repeat:
            break
        cacheable += 1
    return ctx.Assembly(parts, cacheable, tuple(index in kept for index in range(len(active))), omitted_history,
                    content, chars)


