package store

import (
	"math"
	"testing"
)

// 速度口径的三档互斥规则，逐条锁死。
//
// 这些数字不是随便编的：改公式、改门槛、改"从首字算"这条，
// 都会让下面某个用例失败 —— 那正是目的。
func TestComputeOutputSpeed(t *testing.T) {
	intPtr := func(value int) *int { return &value }

	t.Run("经路由：从首字之后算，不标近似", func(t *testing.T) {
		log := &UsageLog{
			CompletionTokens: 500,
			TTFTMs:           intPtr(2000),
			TotalMs:          7000, // 吐字用了 5000ms
		}
		computeOutputSpeed(log)

		if log.TokensPerSecond == nil {
			t.Fatal("应当算出速度")
		}
		// 500 / 5s = 100
		if math.Abs(*log.TokensPerSecond-100) > 0.01 {
			t.Fatalf("速度应为 100，实际 %v", *log.TokensPerSecond)
		}
		if log.SpeedEstimated {
			t.Fatal("经路由的记录不该标近似")
		}
	})

	t.Run("经路由但输出不足 100 不算", func(t *testing.T) {
		log := &UsageLog{
			CompletionTokens: 99,
			TTFTMs:           intPtr(100),
			TotalMs:          500,
		}
		computeOutputSpeed(log)

		if log.TokensPerSecond != nil {
			t.Fatalf("样本太小时不该给速度，实际 %v", *log.TokensPerSecond)
		}
	})

	t.Run("未经路由：用总时长，且标近似", func(t *testing.T) {
		log := &UsageLog{
			CompletionTokens: 400,
			TTFTMs:           nil,
			TotalMs:          4000, // 4s（含等首字）
		}
		computeOutputSpeed(log)

		if log.TokensPerSecond == nil {
			t.Fatal("应当算出近似速度")
		}
		// 400 / 4s = 100
		if math.Abs(*log.TokensPerSecond-100) > 0.01 {
			t.Fatalf("近似速度应为 100，实际 %v", *log.TokensPerSecond)
		}
		if !log.SpeedEstimated {
			t.Fatal("没有首字计时的记录必须标近似")
		}
	})

	t.Run("未经路由但输出不足 200 不算", func(t *testing.T) {
		log := &UsageLog{
			CompletionTokens: 199,
			TTFTMs:           nil,
			TotalMs:          3000,
		}
		computeOutputSpeed(log)

		if log.TokensPerSecond != nil {
			t.Fatalf("近似值的门槛是 200，199 不该算，实际 %v", *log.TokensPerSecond)
		}
	})

	t.Run("被拒绝的请求不算速度", func(t *testing.T) {
		log := &UsageLog{
			CompletionTokens: 500,
			TTFTMs:           intPtr(100),
			TotalMs:          1000,
			IsRejection:      true,
		}
		computeOutputSpeed(log)

		if log.TokensPerSecond != nil {
			t.Fatal("拒绝行没有真实输出，不该有速度")
		}
	})

	t.Run("零输出或零时长不算", func(t *testing.T) {
		for _, log := range []*UsageLog{
			{CompletionTokens: 0, TTFTMs: intPtr(100), TotalMs: 1000},
			{CompletionTokens: 500, TTFTMs: intPtr(100), TotalMs: 0},
			// 总时长等于首字：分母为 0，不能除。
			{CompletionTokens: 500, TTFTMs: intPtr(1000), TotalMs: 1000},
		} {
			computeOutputSpeed(log)
			if log.TokensPerSecond != nil {
				t.Fatalf("不该算出速度：%+v", *log.TokensPerSecond)
			}
		}
	})
}
