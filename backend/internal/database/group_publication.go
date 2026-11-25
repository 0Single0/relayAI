package database

import (
	"relayai/backend/internal/modelpolicy"
	"relayai/backend/internal/models"

	"gorm.io/gorm"
)

// Replace the implicit registry gate with visible, editable rules once. Existing
// explicit allowlists and legacy groups without vendor metadata are untouched.
func migrateGroupPublication(tx *gorm.DB) error {
	var groups []models.Channels
	if err := tx.Where("model_provider <> '' AND COALESCE(array_length(model_allow, 1), 0) = 0").Find(&groups).Error; err != nil {
		return err
	}
	for _, group := range groups {
		rules := modelpolicy.Defaults(group.ModelProvider)
		var known []string
		if err := tx.Model(&models.Models{}).Where("model_provider = ?", group.ModelProvider).Order("canonical_name").Pluck("canonical_name", &known).Error; err != nil {
			return err
		}
		// Preserve previously recognized IDs with nonstandard names too.
		for _, name := range known {
			if len(rules) == 0 || !modelpolicy.Allows(rules, name) {
				rules = append(rules, name)
			}
		}
		if len(rules) == 0 {
			rules = []string{group.ModelProvider + "*"}
		}
		if err := tx.Model(&models.Channels{}).Where("id = ?", group.ID).Update("model_allow", models.TextArray(rules)).Error; err != nil {
			return err
		}
	}
	return nil
}
