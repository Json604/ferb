"""Measure shadow answers against explicit reviewed labels. No permission or configuration changes."""
from typing import Mapping


def measure(labels: Mapping[str, str], answers: Mapping[str, str]) -> dict[str, int | float | None]:
    positive = [key for key, value in labels.items() if value == 'destructive']
    negative = [key for key, value in labels.items() if value == 'safe']
    caught = sum(answers.get(key) == 'destructive' for key in positive)
    false = sum(answers.get(key) == 'destructive' for key in negative)
    return {'labelled': len(labels), 'answered': sum(key in answers for key in labels), 'caught': caught,
            'false_alarms': false, 'catch_rate': caught / len(positive) if positive else None,
            'false_alarm_rate': false / len(negative) if negative else None}
