package database

import "gorm.io/gorm"

// Preserve legacy memberships and IDs. The legacy column is a compatibility
// input for older import/login writers, not the source of routing membership.
func migrateAccountGroups(tx *gorm.DB) error {
	statements := []string{`
ALTER TABLE account_groups
  ADD CONSTRAINT account_groups_account_fkey FOREIGN KEY (account_id) REFERENCES accounts(id) ON DELETE CASCADE,
  ADD CONSTRAINT account_groups_group_fkey FOREIGN KEY (group_id) REFERENCES channels(id) ON DELETE CASCADE,
  ADD CONSTRAINT account_groups_priority_check CHECK (priority IS NULL OR priority >= 0),
  ADD CONSTRAINT account_groups_weight_check CHECK (weight IS NULL OR weight > 0);`, `
INSERT INTO account_groups (account_id, group_id, enabled)
SELECT id, channel_id, true FROM accounts WHERE channel_id IS NOT NULL
ON CONFLICT (account_id, group_id) DO NOTHING;`, `
CREATE OR REPLACE FUNCTION sync_legacy_account_group() RETURNS trigger AS $$
BEGIN
  IF NEW.channel_id IS NOT NULL THEN
    INSERT INTO account_groups (account_id, group_id, enabled)
    VALUES (NEW.id, NEW.channel_id, true)
    ON CONFLICT (account_id, group_id) DO NOTHING;
  END IF;
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;`, `
CREATE TRIGGER accounts_legacy_group
AFTER INSERT OR UPDATE OF channel_id ON accounts
FOR EACH ROW EXECUTE FUNCTION sync_legacy_account_group();
`}
	for _, statement := range statements {
		if err := tx.Exec(statement).Error; err != nil {
			return err
		}
	}
	return nil
}
