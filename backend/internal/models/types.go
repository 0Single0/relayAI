package models

import (
	"database/sql/driver"
	"encoding/json"
	"fmt"

	"gorm.io/gorm"
	"gorm.io/gorm/schema"
)

/*
自定义类型。

为什么需要：
  · `text[]` 和 `jsonb` 是 Postgres 专有类型，GORM 默认映射不出来；
  · 直接用 `[]string` / `map[string]any` 会在扫描时报 "unsupported Scan"。

两个类型都同时实现 `driver.Valuer`（写）与 `sql.Scanner`（读），
再加 `GormDBDataType` 让 AutoMigrate 建出正确的列类型。
*/

// TextArray 映射 Postgres 的 `text[]`。
//
// **nil 与空切片语义不同**：
//   · nil      → 数据库存 NULL，语义是"不限制"；
//   · 空切片   → 数据库存 `{}`，语义是"限制但一个都不允许"。
//
// 调度层依赖这个区别（`model_allow IS NULL` = 不限制），所以序列化时必须保真。
type TextArray []string

// Value 实现 driver.Valuer。
func (a TextArray) Value() (driver.Value, error) {
	if a == nil {
		return nil, nil
	}
	return "{" + joinQuoted(a) + "}", nil
}

// Scan 实现 sql.Scanner。
//
// 直接解析 Postgres 的数组字面量（`{a,b}`）。不引第三方数组解析库：
// 这里只会遇到 `text[]`，且元素是模型名/渠道名（不含花括号与逗号），
// 简单切分足够；真要含特殊字符再做转义解析。
func (a *TextArray) Scan(src any) error {
	if src == nil {
		*a = nil
		return nil
	}
	var raw string
	switch value := src.(type) {
	case string:
		raw = value
	case []byte:
		raw = string(value)
	default:
		return fmt.Errorf("TextArray: 不支持的类型 %T", src)
	}
	*a = splitArray(raw)
	return nil
}

// GormDataType 让 AutoMigrate 建出 `text[]`。
func (TextArray) GormDataType() string { return "text[]" }

// GormDBDataType 显式指定列类型（GORM 要求实现这个接口才能认出数组）。
func (TextArray) GormDBDataType(db *gorm.DB, _ *schema.Field) string {
	switch db.Dialector.Name() {
	case "postgres":
		return "text[]"
	default:
		return "text"
	}
}

// JSONB 映射 Postgres 的 `jsonb`。
type JSONB map[string]any

// Value 实现 driver.Valuer。
func (j JSONB) Value() (driver.Value, error) {
	if j == nil {
		return "{}", nil
	}
	encoded, err := json.Marshal(j)
	if err != nil {
		return nil, fmt.Errorf("JSONB 序列化失败: %w", err)
	}
	return string(encoded), nil
}

// Scan 实现 sql.Scanner。
func (j *JSONB) Scan(src any) error {
	if src == nil {
		*j = JSONB{}
		return nil
	}
	var raw []byte
	switch value := src.(type) {
	case []byte:
		raw = value
	case string:
		raw = []byte(value)
	default:
		return fmt.Errorf("JSONB: 不支持的类型 %T", src)
	}
	if len(raw) == 0 {
		*j = JSONB{}
		return nil
	}
	parsed := JSONB{}
	if err := json.Unmarshal(raw, &parsed); err != nil {
		return fmt.Errorf("JSONB 反序列化失败: %w", err)
	}
	*j = parsed
	return nil
}

// GormDataType 让 AutoMigrate 建出 `jsonb`。
func (JSONB) GormDataType() string { return "jsonb" }

func (JSONB) GormDBDataType(db *gorm.DB, _ *schema.Field) string {
	switch db.Dialector.Name() {
	case "postgres":
		return "jsonb"
	default:
		return "text"
	}
}

// joinQuoted 把元素拼成 `a,b`（外层花括号由调用方加）。
//
// 元素含逗号或花括号时需要加双引号：模型名理论上不会，
// 但 `options` 里可能出现用户自定义的键，先兜住。
func joinQuoted(items []string) string {
	out := ""
	for index, item := range items {
		if index > 0 {
			out += ","
		}
		if needsQuote(item) {
			out += `"` + escape(item) + `"`
		} else {
			out += item
		}
	}
	return out
}

func needsQuote(item string) bool {
	if item == "" {
		return true
	}
	for _, ch := range item {
		switch ch {
		case ',', '{', '}', '"', '\\', ' ':
			return true
		}
	}
	return false
}

func escape(item string) string {
	out := make([]rune, 0, len(item))
	for _, ch := range item {
		if ch == '"' || ch == '\\' {
			out = append(out, '\\')
		}
		out = append(out, ch)
	}
	return string(out)
}

// splitArray 解析 `{a,b}` 形式的数组字面量。
func splitArray(raw string) []string {
	trimmed := raw
	if len(trimmed) >= 2 && trimmed[0] == '{' && trimmed[len(trimmed)-1] == '}' {
		trimmed = trimmed[1 : len(trimmed)-1]
	}
	if trimmed == "" {
		// `{}` 是空数组（非 NULL）。返回空切片而不是 nil，保住语义差别。
		return []string{}
	}

	var out []string
	var current []rune
	quoted := false
	escaped := false
	for _, ch := range trimmed {
		switch {
		case escaped:
			current = append(current, ch)
			escaped = false
		case ch == '\\':
			escaped = true
		case ch == '"':
			quoted = !quoted
		case ch == ',' && !quoted:
			out = append(out, string(current))
			current = nil
		default:
			current = append(current, ch)
		}
	}
	out = append(out, string(current))
	return out
}
