CREATE INDEX token_usage_global_time_idx
    ON token_usage_evidence (
        (substr(observed_at, 1, 19) || '.' || CASE
            WHEN substr(observed_at, 20, 1) = '.'
            THEN replace(printf('%-107s', substr(observed_at, 21, length(observed_at) - 21)), ' ', '0')
            ELSE replace(printf('%-107s', ''), ' ', '0')
        END) DESC,
        event_id DESC
    );

CREATE INDEX quota_snapshot_global_time_idx
    ON quota_snapshot_evidence (
        (substr(observed_at, 1, 19) || '.' || CASE
            WHEN substr(observed_at, 20, 1) = '.'
            THEN replace(printf('%-107s', substr(observed_at, 21, length(observed_at) - 21)), ' ', '0')
            ELSE replace(printf('%-107s', ''), ' ', '0')
        END) DESC,
        sample_id DESC
    );

CREATE INDEX quota_snapshot_meter_time_sort_idx
    ON quota_snapshot_evidence (
        meter_type,
        (substr(observed_at, 1, 19) || '.' || CASE
            WHEN substr(observed_at, 20, 1) = '.'
            THEN replace(printf('%-107s', substr(observed_at, 21, length(observed_at) - 21)), ' ', '0')
            ELSE replace(printf('%-107s', ''), ' ', '0')
        END) DESC,
        sample_id DESC
    );
