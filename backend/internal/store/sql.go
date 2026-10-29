package store

import (
	"context"
	"database/sql"
	"errors"

	"gorm.io/gorm"
)

/*
本文件把"跑一条 SQL 并取结果"收敛成几个小封装。

为什么需要这一层而不是每处各写一遍：
  仓储层有几十处 `Raw(...)` 调用，语义高度一致 ——
  "跑一条 SQL，扫描到目标" 或 "跑一条写语句"。
  逐处写 `Raw(...).Scan(...)` 会把 ErrNoRows / RowsAffected 的处理
  抄几十遍，抄错一次就是一个隐蔽 bug。

与 GORM 的关系：
  这些函数**只是语法糖**，内部走 GORM 的 `Raw` / `Exec` / `Row`，
  连接、日志、事务、预处理语句全部由 GORM 管理，不引入第二套访问机制。
*/

// queryOne 执行一条返回**恰好一行**的 SQL，扫描进 dest。
//
// `dest` 有两种用法，**必须区分**：
//   - 结构体指针 → 用 `Scan`（它按列名映射到字段）；
//   - 单个标量指针（`*int64`、`*float64`）→ 用 `Row().Scan`（按位置扫描）。
//
// 为什么不能统一用 `Row().Scan(dest)`：
//  `sql.Row.Scan` 要求**每个目标参数对应一列**，传结构体会报
//  "expected N destination arguments in Scan, not 1"。
//  而 `gorm.DB.Scan` 在零行时又不报错，无法区分"查不到"。
//
// 所以按类型分流：结构体走 Scan + RowsAffected 判断，
// 标量走 Row().Scan + sql.ErrNoRows 判断。
func (s *Store) queryOne(ctx context.Context, dest any, sqlQuery string, args ...any) error {
	if isStructDest(dest) {
		result := s.db.WithContext(ctx).Raw(sqlQuery, args...).Scan(dest)
		if result.Error != nil {
			return result.Error
		}
		if result.RowsAffected == 0 {
			return ErrNotFound
		}
		return nil
	}

	row := s.db.WithContext(ctx).Raw(sqlQuery, args...).Row()
	if err := row.Scan(dest); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return ErrNotFound
		}
		return err
	}
	return nil
}

// queryAll 执行一条返回多行的 SQL，扫描进 dest（切片指针）。
func (s *Store) queryAll(ctx context.Context, dest any, sqlQuery string, args ...any) error {
	return s.db.WithContext(ctx).Raw(sqlQuery, args...).Scan(dest).Error
}

// queryRows 执行一条返回多行的 SQL，返回可逐行 Scan 的游标。
//
// 给需要**手动逐行处理**的场合用（比如聚合结果里字段是灵活的 JSON）。
// 调用方负责 `defer rows.Close()`。
func (s *Store) queryRows(ctx context.Context, sqlQuery string, args ...any) (*sql.Rows, error) {
	return s.db.WithContext(ctx).Raw(sqlQuery, args...).Rows()
}

// exec 执行写语句，返回受影响行数。
func (s *Store) exec(ctx context.Context, sqlQuery string, args ...any) (int64, error) {
	result := s.db.WithContext(ctx).Exec(sqlQuery, args...)
	return result.RowsAffected, result.Error
}

// execTx 在给定事务里执行写语句。
func execTx(tx *gorm.DB, sqlQuery string, args ...any) (int64, error) {
	result := tx.Exec(sqlQuery, args...)
	return result.RowsAffected, result.Error
}

// isStructDest 判断扫描目标是不是"结构体指针"（含匿名字段的结构也算）。
//
// 只做一级判断：本项目里要么是结构体（含内联结构体字面量），
// 要么是基本类型指针。指针的指针等复杂形态不在此列。
func isStructDest(dest any) bool {
	switch dest.(type) {
	case *int, *int8, *int16, *int32, *int64,
		*uint, *uint8, *uint16, *uint32, *uint64,
		*float32, *float64, *string, *bool,
		*any:
		return false
	}
	// 其余按结构体处理（`*struct{...}` 与具名结构体指针）。
	return true
}
