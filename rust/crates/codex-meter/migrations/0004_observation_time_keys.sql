UPDATE observations
SET started_at = CASE
    WHEN started_at IS NULL THEN NULL
    WHEN instr(started_at, '.') = 0 THEN substr(started_at, 1, 19) || '.000000000Z'
    ELSE substr(started_at, 1, 20) || substr(substr(started_at, instr(started_at, '.') + 1, length(started_at) - instr(started_at, '.') - 1) || '000000000', 1, 9) || 'Z'
END,
ended_at = CASE
    WHEN ended_at IS NULL THEN NULL
    WHEN instr(ended_at, '.') = 0 THEN substr(ended_at, 1, 19) || '.000000000Z'
    ELSE substr(ended_at, 1, 20) || substr(substr(ended_at, instr(ended_at, '.') + 1, length(ended_at) - instr(ended_at, '.') - 1) || '000000000', 1, 9) || 'Z'
END,
finalized_at = CASE
    WHEN finalized_at IS NULL THEN NULL
    WHEN instr(finalized_at, '.') = 0 THEN substr(finalized_at, 1, 19) || '.000000000Z'
    ELSE substr(finalized_at, 1, 20) || substr(substr(finalized_at, instr(finalized_at, '.') + 1, length(finalized_at) - instr(finalized_at, '.') - 1) || '000000000', 1, 9) || 'Z'
END;
