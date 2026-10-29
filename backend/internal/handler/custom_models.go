package handler

import (
	"encoding/json"
	"fmt"
	"math"
	"strings"
)

// Keep legacy ID lists usable while validating rich, account-owned catalogs.
func customModelOptions(raw json.RawMessage, allowed []string) (json.RawMessage, error) {
	options := map[string]json.RawMessage{}
	if len(raw) > 0 && string(raw) != "null" {
		if err := json.Unmarshal(raw, &options); err != nil || options == nil {
			return nil, fmt.Errorf("模型配置必须是对象")
		}
	}
	if _, ok := options["models"]; !ok {
		options["models"], _ = json.Marshal(allowed)
		return json.Marshal(options)
	}
	var entries []json.RawMessage
	if err := json.Unmarshal(options["models"], &entries); err != nil || len(entries) != len(allowed) {
		return nil, fmt.Errorf("模型配置必须与支持模型列表一致")
	}
	wanted := map[string]bool{}
	for _, id := range allowed {
		wanted[id] = true
	}
	seen := map[string]bool{}
	for index, entry := range entries {
		var legacy string
		if json.Unmarshal(entry, &legacy) == nil {
			legacy = strings.TrimSpace(legacy)
			if !wanted[legacy] || seen[legacy] {
				return nil, fmt.Errorf("模型 ID 重复或不在支持模型列表中")
			}
			seen[legacy] = true
			entries[index], _ = json.Marshal(legacy)
			continue
		}
		var model struct {
			ID            string            `json:"id"`
			DisplayName   string            `json:"display_name"`
			Context       *float64          `json:"context_length"`
			Output        *float64          `json:"max_output_tokens"`
			Capabilities  map[string]bool   `json:"capabilities"`
			Efforts       []string          `json:"reasoning_efforts"`
			EffortMap     map[string]string `json:"reasoning_effort_map"`
			DefaultEffort string            `json:"reasoning_default_effort"`
		}
		if err := json.Unmarshal(entry, &model); err != nil {
			return nil, fmt.Errorf("第 %d 个模型配置格式无效", index+1)
		}
		model.ID = strings.TrimSpace(model.ID)
		model.DisplayName = strings.TrimSpace(model.DisplayName)
		if model.ID == "" || strings.ContainsAny(model.ID, " \t\r\n") || !wanted[model.ID] || seen[model.ID] {
			return nil, fmt.Errorf("模型 ID 不能为空、重复、含空格或超出支持模型列表")
		}
		seen[model.ID] = true
		for _, limit := range []*float64{model.Context, model.Output} {
			if limit != nil && (*limit <= 0 || *limit > 9007199254740991 || math.Trunc(*limit) != *limit) {
				return nil, fmt.Errorf("模型 %s 的 token 数必须是正整数", model.ID)
			}
		}
		if model.Context != nil && model.Output != nil && *model.Output > *model.Context {
			return nil, fmt.Errorf("模型 %s 的最大输出不能超过上下文窗口", model.ID)
		}
		levels := map[string]bool{}
		for _, level := range model.Efforts {
			if strings.TrimSpace(level) == "" || strings.TrimSpace(level) != level || levels[level] {
				return nil, fmt.Errorf("模型 %s 的推理等级为空或重复", model.ID)
			}
			levels[level] = true
		}
		for level, value := range model.EffortMap {
			if !levels[level] || strings.TrimSpace(value) == "" {
				return nil, fmt.Errorf("模型 %s 的推理参数映射无效", model.ID)
			}
			model.EffortMap[level] = strings.TrimSpace(value)
		}
		if model.DefaultEffort != "" && !levels[model.DefaultEffort] {
			return nil, fmt.Errorf("默认推理等级必须属于已配置等级")
		}
		entries[index], _ = json.Marshal(model)
	}
	options["models"], _ = json.Marshal(entries)
	return json.Marshal(options)
}
