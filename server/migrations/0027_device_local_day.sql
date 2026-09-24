-- The screen-time ledger is filed under the DEVICE-LOCAL day — the day the
-- agent enforces — not the server's UTC CURRENT_DATE. Agents report their
-- local day with each usage report; this column keeps the device's last
-- reported UTC offset (seconds east of UTC) so the console can tell which
-- date is "today" for a device that hasn't reported yet today.
ALTER TABLE devices ADD COLUMN utc_offset_secs int;
