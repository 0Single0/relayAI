package handler

import (
	"encoding/json"
	"fmt"
	"net/url"
	"strings"

	"relayai/backend/internal/store"
)

func validateAccountUpdate(account store.Account, input *store.AccountInput) error {
	for _, value := range []*int{input.MaxInflight, input.WeightOverride} {
		if value != nil && (*value < 1 || int64(*value) > 2147483647) {
			return fmt.Errorf("并发上限和调度权重必须为正整数，留空恢复默认")
		}
	}
	if input.PriorityOverride != nil && (*input.PriorityOverride < 0 || int64(*input.PriorityOverride) > 2147483647) {
		return fmt.Errorf("调度优先级必须为非负整数，留空恢复默认")
	}
	if len(input.ModelRename) > 0 {
		var names map[string]string
		if err := json.Unmarshal(input.ModelRename, &names); err != nil || names == nil {
			return fmt.Errorf("模型别名必须是模型 ID 到别名的映射")
		}
		for source, target := range names {
			if strings.TrimSpace(source) == "" || strings.TrimSpace(target) == "" || strings.ContainsAny(source+target, " \t\r\n") {
				return fmt.Errorf("模型 ID 和别名不能为空或包含空白字符")
			}
		}
	}
	var options map[string]json.RawMessage
	if len(input.Options) > 0 {
		var patch map[string]json.RawMessage
		if err := json.Unmarshal(input.Options, &patch); err != nil || patch == nil {
			return fmt.Errorf("账号配置必须是对象")
		}
		_ = json.Unmarshal(account.Options, &options)
		if options == nil {
			options = map[string]json.RawMessage{}
		}
		for key, value := range patch {
			if key == "realm" && string(value) != string(options[key]) {
				return fmt.Errorf("账号平台版本不能通过编辑切换，请使用对应版本重新授权")
			}
			options[key] = value
		}
		input.Options, _ = json.Marshal(options)
	}
	platform := account.Platform
	if platform == "" {
		platform = account.Adapter
	}
	if platform != "custom" {
		if input.BaseURL != nil || input.Protocol != nil || input.APIKey != nil {
			return fmt.Errorf("只有自定义供应商可以编辑上游地址、协议和 API Key")
		}
		if options != nil {
			if _, ok := options["models"]; ok {
				return fmt.Errorf("授权平台的模型由上游目录提供")
			}
			if value, ok := options["outbound_identity"]; ok {
				var identity string
				if json.Unmarshal(value, &identity) != nil || (identity != "desktop" && identity != "cli" && identity != "vscode") {
					return fmt.Errorf("无效的出站身份")
				}
			}
		}
		return nil
	}
	if input.BaseURL != nil {
		value := strings.TrimSpace(*input.BaseURL)
		parsed, err := url.Parse(value)
		if err != nil || parsed.Host == "" || (parsed.Scheme != "http" && parsed.Scheme != "https") || parsed.User != nil {
			return fmt.Errorf("请填写有效的 HTTP 或 HTTPS 上游地址")
		}
		input.BaseURL = &value
	}
	if input.Protocol != nil && *input.Protocol != "openai-chat" && *input.Protocol != "openai-responses" && *input.Protocol != "anthropic" {
		return fmt.Errorf("不支持的上游协议")
	}
	if input.ModelAllow != nil || input.Options != nil {
		allowed := account.ModelAllow
		if input.ModelAllow != nil {
			allowed = cleanModelIDs(*input.ModelAllow)
		}
		if len(allowed) == 0 {
			return fmt.Errorf("请至少添加一个支持的模型")
		}
		raw := input.Options
		if raw == nil {
			raw = account.Options
		}
		validated, err := customModelOptions(raw, allowed)
		if err != nil {
			return err
		}
		input.Options = validated
		input.ModelAllow = &allowed
	}
	return nil
}
