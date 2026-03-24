UPDATE branch_inbox_states
SET state = 'inbox',
    reason = 'generation_ready',
    dismissed_at = NULL,
    updated_at = CURRENT_TIMESTAMP
WHERE state = 'dismissed'
  AND reason = 'branch_closed';
