"""Owner controls and receipts for delegated conversations; no model receives these RPC rights."""
import json
from typing import Any


def register(sub: Any) -> None:
    parser = sub.add_parser('conversations', help='delegated conversations: scope, receipts and owner hand-over')
    parser.add_argument('action', nargs='?', default='list', choices=['list', 'show', 'pause', 'resume', 'takeover', 'close'])
    parser.add_argument('id', nargs='?')
    parser.add_argument('--note', default='')
    notices = sub.add_parser('notifications', help='owner notification rules and shadow measurements')
    notices.add_argument('action', nargs='?', default='rules', choices=['rules', 'unmute', 'label'])
    notices.add_argument('id', nargs='?')
    notices.add_argument('--expected', choices=['yes', 'no'])
    notices.add_argument('--note', default='')


def run(client: Any, args: Any, out: Any) -> int:
    if args.cmd == 'conversations':
        if args.action != 'list' and not args.id:
            raise ValueError('A conversation ID is required')
        result = client.call('conversations.list', {'id': args.id}) if args.action in ('list', 'show') else client.call(
            'conversations.change', {'id': args.id, 'action': args.action,
                                     'owner_words': args.note or f'{args.action} conversation {args.id}'})
    elif args.action == 'label':
        if not args.id or not args.expected:
            raise ValueError('A notification event ID and --expected yes|no are required')
        result = client.call('notifications.label', {'event_id': args.id, 'expected': args.expected,
                            'owner_quote': args.note or f'label {args.id} {args.expected}'})
    else:
        if args.action == 'unmute' and not args.id:
            raise ValueError('A rule ID is required')
        result = client.call('notifications.rules', {'remove': args.id if args.action == 'unmute' else None})
    print(json.dumps(result, ensure_ascii=False, indent=2), file=out)
    return 0
