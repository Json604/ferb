"""Visible outcome text, separate from the complete inspectable task report.

Only verified model wording can become a success display. Incomplete outcomes keep
code's status and every unresolved finding; the full condition ledger stays in answer.
"""
from ferb.orchestrator.verify import ConditionResult, UNCHECKED


def outcome_display(status: str, answer: str, checked_display: str, question: str,
                    results: list[ConditionResult], stop_reason: str, recovery_display: str = "") -> str:
    if status == 'cancelled':
        return answer
    if status == 'done':
        return checked_display.strip() or answer
    unresolved = [r for r in results if r.met is not True and r.path != UNCHECKED]
    findings = list(dict.fromkeys(r.evidence + (' How to check: ' + r.how if r.met is None and r.how else '')
                                 for r in unresolved))
    if status == 'needs_user' and question and (findings or not recovery_display.strip()):
        return '\n'.join([question, *findings])
    if findings:
        lead = 'Finished, but not fully confirmed.' if status == 'done_unverified' else "I couldn't confirm completion."
        return '\n'.join([lead, *findings])
    if recovery_display.strip():
        lead = {
            'failed': 'Task incomplete.',
            'needs_user': 'Your answer is needed.',
            'done_unverified': 'Finished, but not fully confirmed.',
        }.get(status, '')
        return (lead + '\n' + recovery_display).strip()
    if stop_reason and status == 'failed':
        return "I couldn't finish: " + stop_reason
    return answer
