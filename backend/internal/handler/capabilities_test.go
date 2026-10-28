package handler

import (
	"encoding/json"
	"testing"

	"relayai/backend/internal/gateway"
)

// 能力字段落库的语义，逐条锁死。
//
// 这些断言保护的是界面上那句话 ——「能力未知」只能表示"真的没数据"，
// 不能把"我们按内置对照表填的"也算进去；否则卡片上就会出现
// 「工具调用 视觉 推理」和「能力未知」并列的自相矛盾。
func TestCapabilitiesOf(t *testing.T) {
	t.Run("实时声明的能力位原样写入", func(t *testing.T) {
		caps := capabilitiesOf(gateway.CatalogCapabilities{
			Vision: true, Tools: true, Reasoning: true, Source: "live",
		})
		for _, key := range []string{"vision", "tools", "reasoning"} {
			if caps[key] != true {
				t.Fatalf("%s 应当为 true，实际 %#v", key, caps[key])
			}
		}
		if caps["capabilitiesSource"] != "live" {
			t.Fatalf("来源应当透传为 live，实际 %#v", caps["capabilitiesSource"])
		}
	})

	t.Run("快照来源与能力位共存", func(t *testing.T) {
		// 这正是 kimi-k2.6 在界面上的情形：有值，但来源是内置对照表。
		caps := capabilitiesOf(gateway.CatalogCapabilities{
			Vision: true, Tools: true, Reasoning: true, Source: "snapshot",
		})
		if caps["capabilitiesSource"] != "snapshot" {
			t.Fatalf("来源应当为 snapshot，实际 %#v", caps["capabilitiesSource"])
		}
		if caps["tools"] != true {
			t.Fatal("快照里的能力位不该被丢掉")
		}
	})

	t.Run("完全没有依据时只有来源一个键", func(t *testing.T) {
		caps := capabilitiesOf(gateway.CatalogCapabilities{Source: "none"})
		if len(caps) != 1 || caps["capabilitiesSource"] != "none" {
			t.Fatalf("未知能力的 JSON 只该有来源字段，实际 %#v", caps)
		}
		if _, exists := caps["capabilitiesKnown"]; exists {
			t.Fatal("旧字段 capabilitiesKnown 不该再写")
		}
	})

	t.Run("不支持的能力位不写键", func(t *testing.T) {
		caps := capabilitiesOf(gateway.CatalogCapabilities{
			Vision: false, Tools: true, Reasoning: false, Source: "live",
		})
		if _, exists := caps["vision"]; exists {
			t.Fatal("false 不该落成键：键不存在才代表没有这条信息")
		}
		if caps["tools"] != true {
			t.Fatal("为真的能力位必须写入")
		}
	})

	t.Run("未知来源不写脏值", func(t *testing.T) {
		caps := capabilitiesOf(gateway.CatalogCapabilities{Source: "garbage"})
		if _, exists := caps["capabilitiesSource"]; exists {
			t.Fatalf("非法来源不该落库，实际 %#v", caps["capabilitiesSource"])
		}
	})

	t.Run("序列化后能被前端读到", func(t *testing.T) {
		raw, err := json.Marshal(capabilitiesOf(gateway.CatalogCapabilities{
			Vision: true, Source: "snapshot",
		}))
		if err != nil {
			t.Fatalf("序列化失败：%v", err)
		}
		var decoded map[string]any
		if err := json.Unmarshal(raw, &decoded); err != nil {
			t.Fatalf("反序列化失败：%v", err)
		}
		if decoded["capabilitiesSource"] != "snapshot" {
			t.Fatalf("JSON 键名必须是 capabilitiesSource，实际 %s", raw)
		}
	})
}
