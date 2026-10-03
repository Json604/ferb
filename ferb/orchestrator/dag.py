"""Read scopes used for independent result inspection, not task routing or permissions."""

READ_SCOPES = frozenset({"fs:read", "browser:read", "mac:read", "screen", "trail:read", "identity:browse",
                         "mailbox:read"})
