CREATE TABLE activity_reports (
  day INTEGER NOT NULL,
  daily_id TEXT NOT NULL,
  platform TEXT NOT NULL,
  version TEXT NOT NULL,
  PRIMARY KEY (day, daily_id)
) WITHOUT ROWID;

CREATE TABLE activity_daily_totals (
  day INTEGER NOT NULL,
  platform TEXT NOT NULL,
  version TEXT NOT NULL,
  devices INTEGER NOT NULL,
  PRIMARY KEY (day, platform, version)
) WITHOUT ROWID;

CREATE TRIGGER count_new_activity_report
AFTER INSERT ON activity_reports
BEGIN
  INSERT INTO activity_daily_totals (day, platform, version, devices)
  VALUES (NEW.day, NEW.platform, NEW.version, 1)
  ON CONFLICT (day, platform, version) DO UPDATE SET devices = devices + 1;
END;
