import type { DailyAction, TaskAccountLive, TaskInfo } from "@/api/types";

/** 一个任务在某个账号下的状态。 */
export type TaskRowAccountState = {
  uid: string;
  nickname: string;
  /** 该账号所属分组；空串 = 待分配（未归组）。 */
  channelId: string;
  /** 该账号下这条任务的状态。 */
  status: TaskInfo["status"];
  progress: string;
  reward: number;
  automatable: boolean;
  note: string;
  enabled: boolean;
  credentialOk: boolean;
};

/** 一行 = 一条任务（跨账号聚合）。 */
export type TaskRow = {
  /** 稳定 key：`daily:<code>` 或 `task:<code>`。 */
  key: string;
  kind: "daily" | "task";
  code: string;
  name: string;
  /** 这条任务在多少个账号下存在。 */
  accountCount: number;
  /** 已完成 / 总数，用于进度。 */
  doneCount: number;
  /** 是否有账号可自动化执行。 */
  automatable: boolean;
  /** 各账号的状态明细（展开时显示）。 */
  accounts: TaskRowAccountState[];
};

/**
 * 把「账号 → 任务」翻成「任务 → 账号」。
 *
 * 为什么必须翻：任务页要回答的是"今天有哪些任务、各自完成到什么程度"，
 * 而不是"这个账号有什么"。按账号列的话，同一个任务在 20 个账号下重复出现 20 次，
 * 用户看不出全局进度。账号退化成**归属维度**（一行里能展开看是哪些号）。
 *
 * 排序：每日动作在前（它们是"今天必须做"的），任务在后；
 * 组内按"未完成优先"——先处理欠账。
 */
export const buildTaskRows = (accounts: TaskAccountLive[]): TaskRow[] => {
  const dailyMap = new Map<string, TaskRow>();
  const taskMap = new Map<string, TaskRow>();

  const push = (
    map: Map<string, TaskRow>,
    kind: TaskRow["kind"],
    code: string,
    name: string,
    account: TaskAccountLive,
    detail: Omit<TaskRowAccountState, "uid" | "nickname" | "channelId" | "enabled" | "credentialOk">,
    done: boolean,
  ) => {
    const key = `${kind}:${code}`;
    let row = map.get(key);
    if (!row) {
      row = {
        key,
        kind,
        code,
        name,
        accountCount: 0,
        doneCount: 0,
        automatable: false,
        accounts: [],
      };
      map.set(key, row);
    }

    row.accountCount += 1;
    if (done) {
      row.doneCount += 1;
    }
    if (detail.automatable) {
      row.automatable = true;
    }
    row.accounts.push({
      uid: account.uid,
      nickname: account.nickname,
      channelId: account.channelId,
      enabled: account.enabled,
      credentialOk: account.credentialOk,
      ...detail,
    });
  };

  for (const account of accounts) {
    for (const action of account.daily as DailyAction[]) {
      push(
        dailyMap,
        "daily",
        action.code,
        action.name,
        account,
        {
          status: action.doneToday ? "done" : "in_progress",
          progress: action.doneToday ? "已完成" : "未完成",
          reward: 0,
          automatable: action.automatable,
          note: action.note,
        },
        action.doneToday,
      );
    }

    for (const task of account.tasks as TaskInfo[]) {
      push(
        taskMap,
        "task",
        task.code,
        task.name,
        account,
        {
          status: task.status,
          progress: task.progress,
          reward: task.reward,
          automatable: task.automatable,
          note: task.note,
        },
        task.status === "done",
      );
    }
  }

  const sortRows = (rows: TaskRow[]) =>
    rows.sort((left, right) => {
      // 未完成优先：欠账排前面。
      const leftDone = left.doneCount === left.accountCount;
      const rightDone = right.doneCount === right.accountCount;
      if (leftDone !== rightDone) {
        return leftDone ? 1 : -1;
      }
      // 可自动化的优先：能一键解决的先给出。
      if (left.automatable !== right.automatable) {
        return left.automatable ? -1 : 1;
      }
      return left.name.localeCompare(right.name, "zh-CN");
    });

  return [...sortRows([...dailyMap.values()]), ...sortRows([...taskMap.values()])];
};

/** 每日动作的完成进度（用于顶部一句话汇总）。 */
export const dailyProgress = (rows: TaskRow[]) => {
  const daily = rows.filter((row) => row.kind === "daily");
  const total = daily.reduce((sum, row) => sum + row.accountCount, 0);
  const done = daily.reduce((sum, row) => sum + row.doneCount, 0);
  return { total, done };
};
