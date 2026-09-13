<script setup lang="ts">
import { computed, markRaw, nextTick, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  Activity,
  AlertCircle,
  Archive,
  ArrowRight,
  ArrowUpCircle,
  Bot,
  Check,
  ChevronRight,
  Clock,
  Code2,
  Coins,
  CreditCard,
  ExternalLink,
  Eye,
  EyeOff,
  FileCode2,
  History,
  Key,
  Layers,
  LayoutDashboard,
  Loader2,
  Lock,
  MinusCircle,
  Monitor,
  Package,
  RefreshCw,
  ShieldCheck,
  Sparkles,
  Tag,
  Terminal,
  Trash2,
  Wallet,
  Wrench,
  X,
} from "@lucide/vue";

type CacheState = "ready" | "partial" | "inUse" | "missing" | "unavailable";
type CategoryId = "all" | "package" | "build" | "tool";
type ActiveView = "home" | "cache" | "prompts" | "updates" | "history" | "deepseek";

interface CacheItem {
  id: string;
  name: string;
  category: Exclude<CategoryId, "all">;
  description: string;
  cleanupNote: string;
  paths: string[];
  sizeBytes: number;
  state: CacheState;
  canClean: boolean;
  blockers: string[];
  scanError: string | null;
}

interface ScanResult {
  items: CacheItem[];
  totalBytes: number;
  reclaimableBytes: number;
  scannedAt: number;
}

interface CleanResult {
  id: string;
  beforeBytes: number;
  remainingBytes: number;
  freedBytes: number;
  skippedEntries: string[];
  message: string;
}

interface CleanupHistoryEntry {
  id: string;
  targetName: string;
  status: "success" | "failed";
  freedBytes: number;
  skippedEntries: string[];
  message: string;
  createdAt: number;
}

interface PromptToolState {
  id: string;
  name: string;
  description: string;
  path: string;
  kind: "file" | "rules";
  exists: boolean;
  usesGlobal: boolean;
  status: "shared" | "personal" | "missing" | "issue";
  message: string;
}

interface PromptManagerState {
  global: {
    enabled: boolean;
    path: string;
    content: string;
  };
  tools: PromptToolState[];
  ompNote: string;
}

interface ToolPromptContent {
  content: string;
  readOnly: boolean;
  exists: boolean;
  files: { name: string }[];
}

interface ToolUpdateInfo {
  id: string;
  name: string;
  installed: boolean;
  currentVersion: string | null;
  latestVersion: string | null;
  status: "latest" | "updateAvailable" | "notInstalled" | "unavailable";
  message: string;
  source: string;
  installMethod?: string | null;
  upgradeCommand?: string | null;
}

interface ToolUpgradeResult {
  id: string;
  name: string;
  success: boolean;
  message: string;
  previousVersion: string | null;
  currentVersion: string | null;
  upgradeCommand: string;
}

interface DeepSeekBalanceInfo {
  currency: string;
  total_balance: string;
  granted_balance: string;
  topped_up_balance: string;
}

interface DeepSeekBalanceResult {
  success: boolean;
  is_available: boolean;
  balance_infos: DeepSeekBalanceInfo[];
  updated_at: number;
  error_message: string | null;
}

const appVersion = "0.1.3";

interface ChangelogEntry {
  version: string;
  date: string;
  isLatest?: boolean;
  highlights: string[];
}

const changelogs: ChangelogEntry[] = [
  {
    version: "0.1.3",
    date: "2026-08-25",
    isLatest: true,
    highlights: [
      "提示词管理：点击开发工具提示词支持直接编辑，保存后自动退出公共规则并保存为专属提示词",
      "工具升级：新增一键全部升级与按工具单独升级，自动检测 pnpm / npm / Homebrew / Bun / Yarn / Cargo 及原生升级命令",
      "界面优化：全局及编辑区移除原生滚动条，保持优雅安静的 macOS 原生质感",
      "新增应用版本与更新日志查看面板",
    ],
  },
  {
    version: "0.1.2",
    date: "2026-08-20",
    highlights: [
      "工具升级检查：支持扫描本机 CLI 工具（Codex、pi、omp、OpenCode、Gemini、Claude Code、grok）的版本状态",
      "提示词管理：集成公共规则与工具专属规则原生路径读写",
    ],
  },
  {
    version: "0.1.1",
    date: "2026-08-15",
    highlights: [
      "垃圾清理：优化包管理器缓存（pnpm、npm、yarn、bun、cargo 等）扫描与安全性校验",
      "清理记录：保留历史审计记录与已释放空间统计",
    ],
  },
  {
    version: "0.1.0",
    date: "2026-08-10",
    highlights: [
      "DevTidy 初始版本发布，提供 macOS 风格的开发环境缓存清理与维护工具",
    ],
  },
];

const scanSnapshotKey = "devtidy.cache-scan.v1";

function readScanSnapshot(): ScanResult | null {
  try {
    const value = window.localStorage.getItem(scanSnapshotKey);
    if (!value) return null;
    const snapshot = JSON.parse(value) as Partial<ScanResult>;
    if (!Array.isArray(snapshot.items) || typeof snapshot.scannedAt !== "number") return null;
    return snapshot as ScanResult;
  } catch {
    return null;
  }
}

function persistScanSnapshot(snapshot: ScanResult) {
  try {
    window.localStorage.setItem(scanSnapshotKey, JSON.stringify(snapshot));
  } catch {
    // 扫描结果仍可在当前会话使用，本地存储不可用时不阻塞清理功能。
  }
}

const categories = [
  { id: "all" as const, label: "全部", icon: markRaw(Layers) },
  { id: "package" as const, label: "包管理器", icon: markRaw(Package) },
  { id: "build" as const, label: "构建产物", icon: markRaw(Code2) },
  { id: "tool" as const, label: "开发工具", icon: markRaw(Wrench) },
];

const cachedScan = readScanSnapshot();
const items = ref<CacheItem[]>(cachedScan?.items ?? []);
const selectedCategory = ref<CategoryId>("all");
const activeView = ref<ActiveView>("home");
const changelogDialog = ref<HTMLDialogElement | null>(null);
const isScanning = ref(false);
const hasScanned = ref(Boolean(cachedScan));
const cleaningId = ref<string | null>(null);
const cleanupHistory = ref<CleanupHistoryEntry[]>([]);
const isHistoryLoading = ref(false);
const isClearingHistory = ref(false);
const scanError = ref("");
const historyError = ref("");
const scannedAt = ref(cachedScan?.scannedAt ?? 0);
const selectedItem = ref<CacheItem | null>(null);
const confirmDialog = ref<HTMLDialogElement | null>(null);
const promptEnableDialog = ref<HTMLDialogElement | null>(null);
const promptManager = ref<PromptManagerState | null>(null);
const selectedPromptToolId = ref("codex");
const selectedGrokFile = ref<string | null>(null);
const toolPrompt = ref<ToolPromptContent | null>(null);
const globalPrompt = ref("");
const promptError = ref("");
const isPromptLoading = ref(false);
const isPromptSaving = ref(false);
const isPromptSwitching = ref(false);
const toolUpdates = ref<ToolUpdateInfo[]>([]);
const isUpdateLoading = ref(false);
const updateError = ref("");
const upgradingToolId = ref<string | null>(null);
const isBatchUpgrading = ref(false);
const batchUpgradeProgress = ref<{ current: number; total: number; toolName: string } | null>(null);
const toast = ref<{ type: "success" | "error"; message: string } | null>(null);
let toastTimer: number | undefined;

const filteredItems = computed(() =>
  selectedCategory.value === "all"
    ? items.value
    : items.value.filter((item) => item.category === selectedCategory.value)
);

const totalBytes = computed(() => items.value.reduce((sum, item) => sum + item.sizeBytes, 0));
const reclaimableBytes = computed(() =>
  items.value.filter((item) => item.canClean).reduce((sum, item) => sum + item.sizeBytes, 0)
);
const busyCount = computed(() => items.value.filter((item) => item.state === "inUse").length);

const packageBytes = computed(() =>
  items.value.filter((i) => i.category === "package").reduce((sum, i) => sum + i.sizeBytes, 0)
);
const buildBytes = computed(() =>
  items.value.filter((i) => i.category === "build").reduce((sum, i) => sum + i.sizeBytes, 0)
);
const toolBytes = computed(() =>
  items.value.filter((i) => i.category === "tool").reduce((sum, i) => sum + i.sizeBytes, 0)
);

const pkgPercent = computed(() =>
  totalBytes.value > 0 ? (packageBytes.value / totalBytes.value) * 100 : 0
);
const buildPercent = computed(() =>
  totalBytes.value > 0 ? (buildBytes.value / totalBytes.value) * 100 : 0
);
const toolPercent = computed(() =>
  totalBytes.value > 0 ? (toolBytes.value / totalBytes.value) * 100 : 0
);

const updateAvailableCount = computed(
  () => toolUpdates.value.filter((t) => t.status === "updateAvailable").length
);

const selectedPromptTool = computed(
  () => promptManager.value?.tools.find((tool) => tool.id === selectedPromptToolId.value) ?? null
);

const viewTitle = computed(
  () =>
    ({
      home: "概览",
      cache: "垃圾清理",
      prompts: "提示词管理",
      updates: "工具升级",
      history: "清理记录",
      deepseek: "DeepSeek 余额",
    })[activeView.value]
);

const viewSubtitle = computed(
  () =>
    ({
      home: "集中维护本机开发缓存、AI 提示词规则与工具版本",
      cache: isScanning.value
        ? "正在实时分析磁盘占用..."
        : scannedAt.value
          ? `上次扫描：${formatTime(scannedAt.value)}`
          : "尚未执行扫描",
      prompts: "公共提示词与工具专属规则均直接保存在本机原生路径",
      updates: isUpdateLoading.value
        ? "正在读取各工具版本与最新发布..."
        : "检查已安装开发工具的最新版本与升级状态",
      history: "保留最近 100 次清理记录与已释放空间审计",
      deepseek: isDeepseekLoading.value
        ? "正在连接 DeepSeek 官方接口查询账户额度..."
        : deepseekBalance.value?.updated_at
          ? `上次同步：${formatTime(deepseekBalance.value.updated_at)}`
          : "实时查询 DeepSeek API 账户可用额度、赠送金与充值明细",
    })[activeView.value]
);

function categoryCount(category: CategoryId) {
  return category === "all"
    ? items.value.length
    : items.value.filter((item) => item.category === category).length;
}

function formatBytes(value: number) {
  if (value <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  const size = value / 1024 ** index;
  return `${size.toFixed(index >= 3 && size < 10 ? 1 : 0)} ${units[index]}`;
}

function toMillis(timestamp: number): number {
  if (!timestamp) return 0;
  return timestamp < 1e11 ? timestamp * 1000 : timestamp;
}

function formatTime(timestamp: number) {
  if (!timestamp) return "尚未扫描";
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  }).format(new Date(toMillis(timestamp)));
}

function formatDateTime(timestamp: number) {
  if (!timestamp) return "未知时间";
  return new Intl.DateTimeFormat(undefined, {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  }).format(new Date(toMillis(timestamp)));
}

function stateLabel(state: CacheState) {
  return {
    ready: "可清理",
    partial: "可部分清理",
    inUse: "正在使用",
    missing: "暂无缓存",
    unavailable: "无法访问",
  }[state];
}

function stateIcon(state: CacheState) {
  return {
    ready: Check,
    partial: ShieldCheck,
    inUse: Lock,
    missing: MinusCircle,
    unavailable: AlertCircle,
  }[state];
}

function itemCategoryIcon(category: CacheItem["category"]) {
  return {
    package: Package,
    build: Code2,
    tool: Wrench,
  }[category];
}

function historyStatusLabel(status: CleanupHistoryEntry["status"]) {
  return status === "success" ? "已完成" : "失败";
}

function promptStatusLabel(status: PromptToolState["status"]) {
  return {
    shared: "公共",
    personal: "专属",
    missing: "未配置",
    issue: "需检查",
  }[status];
}

function updateStatusLabel(status: ToolUpdateInfo["status"]) {
  return {
    latest: "已是最新",
    updateAvailable: "可更新",
    notInstalled: "未安装",
    unavailable: "检查失败",
  }[status];
}

function normalizeError(error: unknown) {
  return typeof error === "string"
    ? error
    : error instanceof Error
      ? error.message
      : "操作失败，请稍后重试";
}

function showToast(type: "success" | "error", message: string) {
  toast.value = { type, message };
  if (toastTimer) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    toast.value = null;
  }, 3600);
}

async function scan() {
  isScanning.value = true;
  scanError.value = "";
  try {
    const result = await invoke<ScanResult>("scan_cache_targets");
    items.value = result.items;
    scannedAt.value = result.scannedAt;
    hasScanned.value = true;
    persistScanSnapshot(result);
  } catch (error) {
    scanError.value = normalizeError(error);
  } finally {
    isScanning.value = false;
  }
}

async function loadCleanupHistory() {
  isHistoryLoading.value = true;
  historyError.value = "";
  try {
    cleanupHistory.value = await invoke<CleanupHistoryEntry[]>("get_cleanup_history");
  } catch (error) {
    historyError.value = normalizeError(error);
  } finally {
    isHistoryLoading.value = false;
  }
}

function navigateTo(view: ActiveView) {
  activeView.value = view;
  if (view === "cache" && !hasScanned.value) {
    void scan();
  } else if (view === "prompts" && !promptManager.value) {
    void loadPromptManager();
  } else if (view === "updates" && !toolUpdates.value.length) {
    void scanToolUpdates();
  } else if (view === "history" && !cleanupHistory.value.length) {
    void loadCleanupHistory();
  }
}

async function clearCleanupHistory() {
  if (!cleanupHistory.value.length || isClearingHistory.value) return;
  isClearingHistory.value = true;
  try {
    await invoke("clear_cleanup_history");
    cleanupHistory.value = [];
    showToast("success", "清理记录已清空");
  } catch (error) {
    showToast("error", normalizeError(error));
  } finally {
    isClearingHistory.value = false;
  }
}

function openChangelog() {
  nextTick(() => changelogDialog.value?.showModal());
}

function closeChangelog() {
  changelogDialog.value?.close();
}

function openConfirm(item: CacheItem) {
  if (!item.canClean || cleaningId.value) return;
  selectedItem.value = item;
  nextTick(() => confirmDialog.value?.showModal());
}

function closeConfirm() {
  if (cleaningId.value) return;
  confirmDialog.value?.close();
  selectedItem.value = null;
}

async function cleanSelected() {
  const target = selectedItem.value;
  if (!target || cleaningId.value) return;
  cleaningId.value = target.id;
  try {
    const result = await invoke<CleanResult>("clean_cache_target", { id: target.id });
    const current = items.value.find((item) => item.id === result.id);
    if (current) {
      current.sizeBytes = result.remainingBytes;
      current.canClean = result.remainingBytes > 0;
      current.state = result.skippedEntries.length ? "partial" : "ready";
    }
    persistScanSnapshot({
      items: items.value,
      totalBytes: totalBytes.value,
      reclaimableBytes: reclaimableBytes.value,
      scannedAt: scannedAt.value,
    });
    confirmDialog.value?.close();
    selectedItem.value = null;
    showToast("success", `已释放 ${formatBytes(result.freedBytes)}`);
  } catch (error) {
    showToast("error", normalizeError(error));
  } finally {
    cleaningId.value = null;
  }
}

async function loadPromptTool() {
  const tool = selectedPromptTool.value;
  if (!tool) return;
  isPromptLoading.value = true;
  promptError.value = "";
  try {
    toolPrompt.value = await invoke<ToolPromptContent>("read_tool_prompt", {
      toolId: tool.id,
      fileName: selectedGrokFile.value,
    });
  } catch (error) {
    toolPrompt.value = null;
    promptError.value = normalizeError(error);
  } finally {
    isPromptLoading.value = false;
  }
}

async function loadPromptManager() {
  isPromptLoading.value = true;
  promptError.value = "";
  try {
    const state = await invoke<PromptManagerState>("get_prompt_manager_state");
    promptManager.value = state;
    globalPrompt.value = state.global.content;
    if (!state.tools.some((tool) => tool.id === selectedPromptToolId.value)) {
      selectedPromptToolId.value = state.tools[0]?.id ?? "codex";
    }
    await loadPromptTool();
  } catch (error) {
    promptError.value = normalizeError(error);
  } finally {
    isPromptLoading.value = false;
  }
}

async function scanToolUpdates() {
  isUpdateLoading.value = true;
  updateError.value = "";
  try {
    toolUpdates.value = await invoke<ToolUpdateInfo[]>("scan_tool_updates");
  } catch (error) {
    updateError.value = normalizeError(error);
  } finally {
    isUpdateLoading.value = false;
  }
}

async function upgradeSingleTool(tool: ToolUpdateInfo) {
  if (upgradingToolId.value || isBatchUpgrading.value || isUpdateLoading.value) return;
  upgradingToolId.value = tool.id;
  try {
    const result = await invoke<ToolUpgradeResult>("upgrade_tool", { toolId: tool.id });
    if (result.success) {
      showToast("success", `${tool.name} ${result.message}`);
    } else {
      showToast("error", `${tool.name} ${result.message}`);
    }
    await scanToolUpdates();
  } catch (error) {
    showToast("error", normalizeError(error));
  } finally {
    upgradingToolId.value = null;
  }
}

async function upgradeAllAvailableTools() {
  if (upgradingToolId.value || isBatchUpgrading.value || isUpdateLoading.value) return;
  const available = toolUpdates.value.filter((t) => t.status === "updateAvailable");
  if (!available.length) return;

  isBatchUpgrading.value = true;
  let successCount = 0;
  let failCount = 0;

  try {
    for (let index = 0; index < available.length; index++) {
      const tool = available[index];
      batchUpgradeProgress.value = {
        current: index + 1,
        total: available.length,
        toolName: tool.name,
      };
      try {
        const result = await invoke<ToolUpgradeResult>("upgrade_tool", { toolId: tool.id });
        if (result.success) {
          successCount++;
        } else {
          failCount++;
        }
      } catch {
        failCount++;
      }
    }

    if (failCount === 0) {
      showToast("success", `已成功升级全部 ${successCount} 个工具`);
    } else if (successCount > 0) {
      showToast("success", `已完成升级：${successCount} 个成功，${failCount} 个失败`);
    } else {
      showToast("error", `工具升级失败，请检查各工具安装环境`);
    }

    await scanToolUpdates();
  } catch (error) {
    showToast("error", normalizeError(error));
  } finally {
    isBatchUpgrading.value = false;
    batchUpgradeProgress.value = null;
  }
}

function selectPromptTool(toolId: string) {
  selectedPromptToolId.value = toolId;
  selectedGrokFile.value = null;
  void loadPromptTool();
}

function selectGrokFile(fileName: string | null) {
  selectedGrokFile.value = fileName;
  void loadPromptTool();
}

async function saveGlobalPrompt() {
  isPromptSaving.value = true;
  try {
    await invoke("save_global_prompt", { content: globalPrompt.value });
    if (promptManager.value) promptManager.value.global.content = globalPrompt.value;
    showToast("success", "公共提示词已保存");
  } catch (error) {
    showToast("error", normalizeError(error));
  } finally {
    isPromptSaving.value = false;
  }
}

async function saveToolPrompt() {
  const tool = selectedPromptTool.value;
  if (!tool || !toolPrompt.value) return;
  const wasUsingGlobal = tool.usesGlobal;
  isPromptSaving.value = true;
  try {
    await invoke("save_tool_prompt", {
      toolId: tool.id,
      fileName: selectedGrokFile.value,
      content: toolPrompt.value.content,
    });
    if (tool.kind === "rules" && !selectedGrokFile.value) {
      selectedGrokFile.value = "dev-cache-cleaner.md";
    }
    await loadPromptManager();
    showToast(
      "success",
      wasUsingGlobal
        ? `${tool.name} 已退出公共提示词并保存为专属提示词`
        : `${tool.name} 专属提示词已保存`
    );
  } catch (error) {
    showToast("error", normalizeError(error));
  } finally {
    isPromptSaving.value = false;
  }
}

function requestGlobalToggle() {
  if (!promptManager.value || isPromptSwitching.value) return;
  if (promptManager.value.global.enabled) {
    void setGlobalPromptEnabled(false);
    return;
  }
  nextTick(() => promptEnableDialog.value?.showModal());
}

async function setGlobalPromptEnabled(enabled: boolean) {
  isPromptSwitching.value = true;
  try {
    await invoke("set_global_prompt_enabled", { enabled });
    promptEnableDialog.value?.close();
    await loadPromptManager();
    showToast("success", enabled ? "公共提示词已启用" : "已恢复专属提示词");
  } catch (error) {
    showToast("error", normalizeError(error));
  } finally {
    isPromptSwitching.value = false;
  }
}

async function toggleToolGlobal() {
  const tool = selectedPromptTool.value;
  if (!tool || !promptManager.value?.global.enabled || isPromptSwitching.value) return;
  isPromptSwitching.value = true;
  try {
    await invoke("set_tool_global_prompt_enabled", {
      toolId: tool.id,
      enabled: !tool.usesGlobal,
    });
    await loadPromptManager();
    showToast(
      "success",
      !tool.usesGlobal ? `${tool.name} 已切换为公共提示词` : `${tool.name} 已恢复专属提示词`
    );
  } catch (error) {
    showToast("error", normalizeError(error));
  } finally {
    isPromptSwitching.value = false;
  }
}

// ---------------------------------------------------------------------------
// DeepSeek Balance Management
// ---------------------------------------------------------------------------
const deepseekApiKey = ref("");
const inputApiKey = ref("");
const showApiKey = ref(false);
const deepseekBalance = ref<DeepSeekBalanceResult | null>(null);
const isDeepseekLoading = ref(false);
const isSavingApiKey = ref(false);
const isKeyFromEnv = ref(false);
const showTrayBalance = ref(false);
const isTogglingTray = ref(false);

interface AutoSyncConfig {
  enabled: boolean;
  interval_minutes: number;
}

const autoSyncEnabled = ref(false);
const autoSyncInterval = ref<number>(30);
const isUpdatingAutoSync = ref(false);

async function loadAutoSyncConfig() {
  try {
    const cfg = await invoke<AutoSyncConfig>("get_deepseek_auto_sync_config");
    autoSyncEnabled.value = cfg.enabled;
    autoSyncInterval.value = cfg.interval_minutes;
  } catch (error) {
    console.error("读取自动同步设置失败：", error);
  }
}

async function toggleAutoSync() {
  if (isUpdatingAutoSync.value) return;
  isUpdatingAutoSync.value = true;
  const target = !autoSyncEnabled.value;
  try {
    const res = await invoke<AutoSyncConfig>("set_deepseek_auto_sync_config", {
      enabled: target,
      intervalMinutes: autoSyncInterval.value,
    });
    autoSyncEnabled.value = res.enabled;
    autoSyncInterval.value = res.interval_minutes;
    showToast(
      "success",
      target
        ? `已开启后台自动同步（每 ${res.interval_minutes === 60 ? '1 小时' : '30 分钟'}）`
        : "已关闭后台自动同步"
    );
  } catch (error) {
    showToast("error", normalizeError(error));
  } finally {
    isUpdatingAutoSync.value = false;
  }
}

async function setAutoSyncInterval(minutes: number) {
  if (isUpdatingAutoSync.value || autoSyncInterval.value === minutes) return;
  isUpdatingAutoSync.value = true;
  try {
    const res = await invoke<AutoSyncConfig>("set_deepseek_auto_sync_config", {
      enabled: autoSyncEnabled.value,
      intervalMinutes: minutes,
    });
    autoSyncEnabled.value = res.enabled;
    autoSyncInterval.value = res.interval_minutes;
    showToast("success", `已设置自动同步周期为 ${minutes === 60 ? '1 小时' : '30 分钟'}`);
  } catch (error) {
    showToast("error", normalizeError(error));
  } finally {
    isUpdatingAutoSync.value = false;
  }
}

async function loadTraySwitch() {
  try {
    const enabled = await invoke<boolean>("get_deepseek_tray_switch");
    showTrayBalance.value = enabled;
  } catch (error) {
    console.error("读取状态栏设置失败：", error);
  }
}

async function toggleTraySwitch() {
  if (isTogglingTray.value) return;
  isTogglingTray.value = true;
  const target = !showTrayBalance.value;
  try {
    await invoke("set_deepseek_tray_switch", { enabled: target });
    showTrayBalance.value = target;
    showToast(
      "success",
      target
        ? "已开启 macOS 状态栏实时余额显示"
        : "已关闭 macOS 状态栏余额显示"
    );
  } catch (error) {
    showToast("error", normalizeError(error));
  } finally {
    isTogglingTray.value = false;
  }
}

interface ApiKeyInfo {
  api_key: string | null;
  is_from_env: boolean;
}

async function loadDeepSeekApiKey() {
  try {
    const res = await invoke<ApiKeyInfo>("get_deepseek_api_key");
    if (res?.api_key) {
      deepseekApiKey.value = res.api_key;
      inputApiKey.value = res.api_key;
      isKeyFromEnv.value = res.is_from_env;
      await fetchDeepSeekBalance(res.api_key, true);
    }
  } catch (error) {
    console.error("读取 DeepSeek 配置失败：", error);
  }
}

async function saveDeepSeekKey() {
  const trimmed = inputApiKey.value.trim();
  if (!trimmed) {
    showToast("error", "请输入有效的 DeepSeek API Key");
    return;
  }
  isSavingApiKey.value = true;
  try {
    await invoke("save_deepseek_api_key", { apiKey: trimmed });
    deepseekApiKey.value = trimmed;
    isKeyFromEnv.value = false;
    showToast("success", "DeepSeek API Key 保存成功");
    await fetchDeepSeekBalance(trimmed);
  } catch (error) {
    showToast("error", normalizeError(error));
  } finally {
    isSavingApiKey.value = false;
  }
}

async function clearDeepSeekKey() {
  try {
    await invoke("clear_deepseek_api_key");
    deepseekApiKey.value = "";
    inputApiKey.value = "";
    isKeyFromEnv.value = false;
    deepseekBalance.value = null;
    showToast("success", "已清除 DeepSeek API Key");
  } catch (error) {
    showToast("error", normalizeError(error));
  }
}

async function fetchDeepSeekBalance(apiKeyOverride?: string, silentSuccess = false) {
  isDeepseekLoading.value = true;
  try {
    const res = await invoke<DeepSeekBalanceResult>("fetch_deepseek_balance", {
      apiKey: apiKeyOverride ?? (inputApiKey.value.trim() || undefined),
    });
    deepseekBalance.value = res;
    if (res.success) {
      if (!silentSuccess) {
        showToast("success", "DeepSeek 余额同步成功");
      }
    } else if (res.error_message && !silentSuccess) {
      showToast("error", res.error_message);
    }
  } catch (error) {
    const msg = normalizeError(error);
    deepseekBalance.value = {
      success: false,
      is_available: false,
      balance_infos: [],
      updated_at: Math.floor(Date.now() / 1000),
      error_message: msg,
    };
    if (!silentSuccess) {
      showToast("error", msg);
    }
  } finally {
    isDeepseekLoading.value = false;
  }
}

function openExternalLink(url: string) {
  void openUrl(url);
}

onMounted(() => {
  if (!hasScanned.value) {
    void scan();
  }
  void loadDeepSeekApiKey();
  void loadTraySwitch();
  void loadAutoSyncConfig();
  void listen<DeepSeekBalanceResult>("deepseek-balance-updated", (event) => {
    if (event.payload) {
      deepseekBalance.value = event.payload;
    }
  });
});
</script>

<template>
  <div class="app-shell">
    <!-- macOS Unified Master Navigation Sidebar -->
    <aside class="sidebar" aria-label="应用导航">
      <div class="sidebar-header">
        <img src="/devtidy-icon.png" alt="DevTidy" class="app-brand-icon" />
        <div class="app-brand-text">
          <span class="app-brand-title">DevTidy</span>
          <button class="app-brand-version-btn" type="button" @click="openChangelog">
            <Tag :size="10" :stroke-width="1.75" />
            <span>v{{ appVersion }}</span>
          </button>
        </div>
      </div>

      <nav class="sidebar-nav">
        <button
          class="nav-item"
          :class="{ active: activeView === 'home' }"
          type="button"
          @click="navigateTo('home')"
        >
          <span class="nav-icon"><LayoutDashboard :size="16" :stroke-width="1.75" /></span>
          <span class="nav-label">概览</span>
        </button>

        <button
          class="nav-item"
          :class="{ active: activeView === 'cache' }"
          type="button"
          @click="navigateTo('cache')"
        >
          <span class="nav-icon"><Trash2 :size="16" :stroke-width="1.75" /></span>
          <span class="nav-label">垃圾清理</span>
          <span v-if="reclaimableBytes > 0" class="nav-badge">{{ formatBytes(reclaimableBytes) }}</span>
        </button>

        <button
          class="nav-item"
          :class="{ active: activeView === 'prompts' }"
          type="button"
          @click="navigateTo('prompts')"
        >
          <span class="nav-icon"><FileCode2 :size="16" :stroke-width="1.75" /></span>
          <span class="nav-label">提示词管理</span>
        </button>

        <button
          class="nav-item"
          :class="{ active: activeView === 'updates' }"
          type="button"
          @click="navigateTo('updates')"
        >
          <span class="nav-icon"><ArrowUpCircle :size="16" :stroke-width="1.75" /></span>
          <span class="nav-label">工具升级</span>
          <span v-if="updateAvailableCount > 0" class="nav-dot" />
        </button>

        <button
          class="nav-item"
          :class="{ active: activeView === 'history' }"
          type="button"
          @click="navigateTo('history')"
        >
          <span class="nav-icon"><History :size="16" :stroke-width="1.75" /></span>
          <span class="nav-label">清理记录</span>
          <span v-if="cleanupHistory.length" class="nav-badge">{{ cleanupHistory.length }}</span>
        </button>

        <button
          class="nav-item"
          :class="{ active: activeView === 'deepseek' }"
          type="button"
          @click="navigateTo('deepseek')"
        >
          <span class="nav-icon"><Bot :size="16" :stroke-width="1.75" /></span>
          <span class="nav-label">DeepSeek 余额</span>
          <span
            v-if="deepseekBalance && deepseekBalance.success && deepseekBalance.balance_infos.length"
            class="nav-badge"
          >
            {{ deepseekBalance.balance_infos[0].currency === 'CNY' ? '¥' : '$' }}{{ deepseekBalance.balance_infos[0].total_balance }}
          </span>
        </button>
      </nav>

      <div class="sidebar-footer">
        <button class="sidebar-version-action-btn" type="button" @click="openChangelog">
          <Sparkles :size="13" :stroke-width="1.75" />
          <span>v{{ appVersion }} 更新日志</span>
        </button>

        <div class="sidebar-status-card">
          <ShieldCheck :size="16" :stroke-width="1.75" />
          <div>
            <strong>安全边界保护</strong>
            <p>操作前校验本机进程与固定安全白名单</p>
          </div>
        </div>
      </div>
    </aside>

    <!-- Main Workspace Area -->
    <main class="main-workspace">
      <!-- Window Toolbar Header -->
      <header class="window-toolbar">
        <div class="toolbar-title-group">
          <h1>{{ viewTitle }}</h1>
          <span class="toolbar-subtitle">{{ viewSubtitle }}</span>
        </div>

        <div class="toolbar-actions">
          <template v-if="activeView === 'cache'">
            <div class="segmented-control">
              <button
                v-for="cat in categories"
                :key="cat.id"
                class="segmented-option"
                :class="{ active: selectedCategory === cat.id }"
                type="button"
                @click="selectedCategory = cat.id"
              >
                <component :is="cat.icon" :size="13" :stroke-width="1.75" />
                <span>{{ cat.label }}</span>
                <span class="segmented-option-count">{{ categoryCount(cat.id) }}</span>
              </button>
            </div>
            <button
              class="btn btn-secondary"
              type="button"
              :disabled="isScanning"
              @click="scan"
            >
              <RefreshCw :class="{ spinning: isScanning }" :size="13" :stroke-width="1.75" />
              <span>重新扫描</span>
            </button>
          </template>

          <template v-else-if="activeView === 'prompts'">
            <button
              class="btn btn-secondary"
              type="button"
              :disabled="isPromptLoading || isPromptSwitching"
              @click="loadPromptManager"
            >
              <RefreshCw :class="{ spinning: isPromptLoading }" :size="13" :stroke-width="1.75" />
              <span>重新读取</span>
            </button>
          </template>

          <template v-else-if="activeView === 'updates'">
            <button
              v-if="updateAvailableCount > 0"
              class="btn btn-primary"
              type="button"
              :disabled="isUpdateLoading || isBatchUpgrading || upgradingToolId !== null"
              @click="upgradeAllAvailableTools"
            >
              <Loader2 v-if="isBatchUpgrading" class="spinning" :size="13" :stroke-width="1.75" />
              <ArrowUpCircle v-else :size="13" :stroke-width="1.75" />
              <span>
                {{
                  isBatchUpgrading && batchUpgradeProgress
                    ? `正在升级 (${batchUpgradeProgress.current}/${batchUpgradeProgress.total})`
                    : `一键升级 (${updateAvailableCount})`
                }}
              </span>
            </button>
            <button
              class="btn btn-secondary"
              type="button"
              :disabled="isUpdateLoading || isBatchUpgrading || upgradingToolId !== null"
              @click="scanToolUpdates"
            >
              <RefreshCw :class="{ spinning: isUpdateLoading }" :size="13" :stroke-width="1.75" />
              <span>检查更新</span>
            </button>
          </template>

          <template v-else-if="activeView === 'history'">
            <button
              class="btn btn-secondary"
              type="button"
              :disabled="!cleanupHistory.length || isClearingHistory"
              @click="clearCleanupHistory"
            >
              <Trash2 :size="13" :stroke-width="1.75" />
              <span>清空记录</span>
            </button>
          </template>
          <template v-else-if="activeView === 'deepseek'">
            <button
              class="btn btn-secondary"
              type="button"
              :disabled="isDeepseekLoading"
              @click="fetchDeepSeekBalance()"
            >
              <RefreshCw :class="{ spinning: isDeepseekLoading }" :size="13" :stroke-width="1.75" />
              <span>刷新余额</span>
            </button>
          </template>
        </div>
      </header>

      <!-- View: Overview / Dashboard -->
      <div v-if="activeView === 'home'" class="workspace-scrollable">
        <div class="dashboard-grid">
          <div class="dashboard-hero-card">
            <div class="dashboard-hero-info">
              <span class="dashboard-hero-label">当前可安全清理空间</span>
              <span class="dashboard-hero-value">{{ formatBytes(reclaimableBytes) }}</span>
              <span class="dashboard-hero-desc">
                已扫描 {{ items.length }} 项缓存（共计 {{ formatBytes(totalBytes) }}），其中 {{ busyCount }} 项正在被进程使用
              </span>
            </div>
            <button
              class="btn btn-primary"
              type="button"
              :disabled="isScanning"
              @click="navigateTo('cache')"
            >
              <span>查看缓存详情</span>
              <ArrowRight :size="14" :stroke-width="1.75" />
            </button>
          </div>

          <h2 class="dashboard-section-title">功能模块</h2>

          <div class="dashboard-cards-row">
            <div class="dashboard-card" @click="navigateTo('cache')">
              <div class="dashboard-card-top">
                <div class="dashboard-card-icon">
                  <Trash2 :size="16" :stroke-width="1.75" />
                </div>
                <ChevronRight :size="14" :stroke-width="1.75" />
              </div>
              <div class="dashboard-card-bottom">
                <strong>垃圾清理</strong>
                <small>包管理器与构建缓存安全释放</small>
              </div>
            </div>

            <div class="dashboard-card" @click="navigateTo('prompts')">
              <div class="dashboard-card-top">
                <div class="dashboard-card-icon">
                  <FileCode2 :size="16" :stroke-width="1.75" />
                </div>
                <ChevronRight :size="14" :stroke-width="1.75" />
              </div>
              <div class="dashboard-card-bottom">
                <strong>提示词管理</strong>
                <small>统一维护各开发工具规则配置</small>
              </div>
            </div>

            <div class="dashboard-card" @click="navigateTo('updates')">
              <div class="dashboard-card-top">
                <div class="dashboard-card-icon">
                  <ArrowUpCircle :size="16" :stroke-width="1.75" />
                </div>
                <ChevronRight :size="14" :stroke-width="1.75" />
              </div>
              <div class="dashboard-card-bottom">
                <strong>工具升级</strong>
                <small>检查已安装 CLI 工具最新版本</small>
              </div>
            </div>

            <div class="dashboard-card" @click="navigateTo('deepseek')">
              <div class="dashboard-card-top">
                <div class="dashboard-card-icon">
                  <Bot :size="16" :stroke-width="1.75" />
                </div>
                <ChevronRight :size="14" :stroke-width="1.75" />
              </div>
              <div class="dashboard-card-bottom">
                <strong>DeepSeek 余额</strong>
                <small v-if="deepseekBalance && deepseekBalance.success && deepseekBalance.balance_infos.length">
                  {{ deepseekBalance.balance_infos[0].currency === 'CNY' ? '¥' : '$' }}{{ deepseekBalance.balance_infos[0].total_balance }} · {{ deepseekBalance.is_available ? '可用' : '欠费' }}
                </small>
                <small v-else-if="deepseekApiKey">已配置 Key，点击查询</small>
                <small v-else>未配置 API Key</small>
              </div>
            </div>

            <div class="dashboard-card" @click="openChangelog">
              <div class="dashboard-card-top">
                <div class="dashboard-card-icon">
                  <Sparkles :size="16" :stroke-width="1.75" />
                </div>
                <ChevronRight :size="14" :stroke-width="1.75" />
              </div>
              <div class="dashboard-card-bottom">
                <strong>更新日志</strong>
                <small>v{{ appVersion }} 版本记录与特性</small>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- View: Cache Cleaner -->
      <div v-else-if="activeView === 'cache'" class="workspace-scrollable">
        <section class="cache-overview-strip" aria-label="容量概览">
          <div class="cache-summary-header">
            <div class="cache-summary-stats">
              <div class="cache-stat-item highlight">
                <span>可安全释放</span>
                <strong>{{ formatBytes(reclaimableBytes) }}</strong>
              </div>
              <div class="cache-stat-item">
                <span>总已扫描缓存</span>
                <strong>{{ formatBytes(totalBytes) }}</strong>
              </div>
              <div class="cache-stat-item">
                <span>占用保护中</span>
                <strong>{{ busyCount }} 项</strong>
              </div>
            </div>
          </div>

          <div class="storage-bar-wrapper">
            <div class="storage-bar-track">
              <div class="storage-bar-segment pkg" :style="{ width: `${pkgPercent}%` }" />
              <div class="storage-bar-segment build" :style="{ width: `${buildPercent}%` }" />
              <div class="storage-bar-segment tool" :style="{ width: `${toolPercent}%` }" />
            </div>
            <div class="storage-bar-legend">
              <div class="legend-item">
                <span class="legend-dot pkg" />
                <span>包管理器 {{ formatBytes(packageBytes) }}</span>
              </div>
              <div class="legend-item">
                <span class="legend-dot build" />
                <span>构建产物 {{ formatBytes(buildBytes) }}</span>
              </div>
              <div class="legend-item">
                <span class="legend-dot tool" />
                <span>开发工具 {{ formatBytes(toolBytes) }}</span>
              </div>
            </div>
          </div>
        </section>

        <!-- Cache Items Table -->
        <section class="table-container">
          <div class="table-header-bar">
            <span class="dashboard-section-title">缓存项目清单</span>
            <span class="toolbar-subtitle">{{ filteredItems.length }} 项</span>
          </div>

          <div v-if="isScanning && !items.length" class="skeleton-list">
            <div v-for="index in 5" :key="index" class="skeleton-row" />
          </div>

          <div v-else-if="scanError" class="empty-state-box">
            <AlertCircle :size="24" :stroke-width="1.75" />
            <h3>扫描未完成</h3>
            <p>{{ scanError }}</p>
            <button class="btn btn-secondary" type="button" @click="scan">重试扫描</button>
          </div>

          <div v-else-if="!filteredItems.length" class="empty-state-box">
            <Archive :size="24" :stroke-width="1.75" />
            <h3>当前分类暂无缓存项目</h3>
            <p>可切换其他分类或点击右上角重新扫描。</p>
          </div>

          <div v-else class="table-list">
            <article v-for="item in filteredItems" :key="item.id" class="cache-row-item">
              <div class="row-icon-cell" aria-hidden="true">
                <component :is="itemCategoryIcon(item.category)" :size="16" :stroke-width="1.75" />
              </div>

              <div class="row-info-cell">
                <div class="row-name-line">
                  <h3>{{ item.name }}</h3>
                  <span class="badge" :class="`badge-${item.state}`">
                    <component :is="stateIcon(item.state)" :size="11" :stroke-width="1.75" />
                    {{ stateLabel(item.state) }}
                  </span>
                </div>
                <p class="row-desc">{{ item.description }}</p>
                <div class="row-paths-pill" :title="item.paths.join('\n')">
                  {{ item.paths.join('  |  ') }}
                </div>
                <p v-if="item.blockers.length" class="row-blocker-note">
                  <Activity :size="11" :stroke-width="1.75" />
                  {{ item.state === 'partial' ? '保留占用条目' : '占用进程' }}：{{ item.blockers.join('、') }}
                </p>
              </div>

              <div class="row-size-cell">
                {{ formatBytes(item.sizeBytes) }}
              </div>

              <div class="row-action-cell">
                <button
                  class="btn btn-secondary btn-sm"
                  type="button"
                  :disabled="!item.canClean || Boolean(cleaningId)"
                  @click="openConfirm(item)"
                >
                  <Loader2 v-if="cleaningId === item.id" class="spinning" :size="12" :stroke-width="1.75" />
                  <Trash2 v-else :size="12" :stroke-width="1.75" />
                  <span>清理</span>
                </button>
              </div>
            </article>
          </div>
        </section>
      </div>

      <!-- View: Prompts Manager (Master-Detail Split View) -->
      <div v-else-if="activeView === 'prompts'" class="prompt-split-workspace">
        <!-- Sub-Sidebar: Tools & Global Prompts -->
        <aside class="prompt-sidebar-pane" aria-label="开发工具规则列表">
          <span class="prompt-pane-title">公共规则</span>
          <button
            class="prompt-tool-btn"
            :class="{ active: selectedPromptToolId === '__global__' }"
            type="button"
            @click="selectedPromptToolId = '__global__'"
          >
            <FileCode2 :size="16" :stroke-width="1.75" />
            <div class="prompt-tool-info">
              <span class="prompt-tool-name">公共提示词</span>
              <span class="prompt-tool-path">global.md</span>
            </div>
            <span
              class="prompt-tool-badge"
              :class="promptManager?.global.enabled ? 'shared' : 'personal'"
            >
              {{ promptManager?.global.enabled ? '已开启' : '未开启' }}
            </span>
          </button>

          <span class="prompt-pane-title" style="margin-top: 8px;">开发工具</span>
          <template v-if="promptManager">
            <button
              v-for="tool in promptManager.tools"
              :key="tool.id"
              class="prompt-tool-btn"
              :class="{ active: selectedPromptToolId === tool.id }"
              type="button"
              @click="selectPromptTool(tool.id)"
            >
              <Terminal :size="15" :stroke-width="1.75" />
              <div class="prompt-tool-info">
                <span class="prompt-tool-name">{{ tool.name }}</span>
                <span class="prompt-tool-path">{{ tool.path.split('/').pop() }}</span>
              </div>
              <span class="prompt-tool-badge" :class="tool.status">
                {{ promptStatusLabel(tool.status) }}
              </span>
            </button>
          </template>
        </aside>

        <!-- Main Detail Editor Pane -->
        <section class="prompt-editor-pane">
          <!-- Global Prompt Editor -->
          <template v-if="selectedPromptToolId === '__global__'">
            <div class="editor-header-bar">
              <div class="editor-header-left">
                <span class="editor-title">公共提示词 (Global Prompts)</span>
                <span class="editor-path-code">{{ promptManager?.global.path }}</span>
              </div>
              <div class="toolbar-actions">
                <button
                  class="switch-control"
                  :class="{ active: promptManager?.global.enabled }"
                  type="button"
                  role="switch"
                  :aria-checked="promptManager?.global.enabled"
                  :disabled="isPromptSwitching"
                  @click="requestGlobalToggle"
                >
                  <span class="switch-track" />
                  <span class="switch-label">
                    {{ promptManager?.global.enabled ? '已开启共享' : '未开启共享' }}
                  </span>
                </button>
              </div>
            </div>

            <textarea
              v-model="globalPrompt"
              class="code-editor-textarea"
              placeholder="公共提示词默认为空。开启共享后，关联的工具将读取这里的内容..."
              spellcheck="false"
            />

            <div class="editor-footer-bar">
              <span>
                {{
                  promptManager?.global.enabled
                    ? `正在共享给 ${promptManager.tools.filter((t) => t.usesGlobal).length} 个工具`
                    : '开启共享后，工具将优先读取公共提示词'
                }}
              </span>
              <button
                class="btn btn-primary btn-sm"
                type="button"
                :disabled="isPromptSaving"
                @click="saveGlobalPrompt"
              >
                <Loader2 v-if="isPromptSaving" class="spinning" :size="12" :stroke-width="1.75" />
                <Check v-else :size="12" :stroke-width="1.75" />
                <span>保存公共提示词</span>
              </button>
            </div>
          </template>

          <!-- Specific Tool Editor -->
          <template v-else-if="selectedPromptTool">
            <div class="editor-header-bar">
              <div class="editor-header-left">
                <span class="editor-title">{{ selectedPromptTool.name }}</span>
                <span class="editor-path-code">{{ selectedPromptTool.path }}</span>
              </div>
              <div class="toolbar-actions">
                <button
                  v-if="promptManager?.global.enabled"
                  class="btn btn-secondary btn-sm"
                  type="button"
                  :disabled="isPromptSwitching || selectedPromptTool.status === 'issue'"
                  @click="toggleToolGlobal"
                >
                  <Loader2 v-if="isPromptSwitching" class="spinning" :size="12" :stroke-width="1.75" />
                  <span v-else>
                    {{ selectedPromptTool.usesGlobal ? '退出公共提示词' : '使用公共提示词' }}
                  </span>
                </button>
              </div>
            </div>

            <!-- Grok Rule Files Switcher -->
            <div v-if="selectedPromptTool.kind === 'rules'" class="editor-tab-bar">
              <button
                class="editor-tab"
                :class="{ active: !selectedGrokFile }"
                type="button"
                @click="selectGrokFile(null)"
              >
                dev-cache-cleaner.md
              </button>
              <button
                v-for="file in toolPrompt?.files"
                :key="file.name"
                class="editor-tab"
                :class="{ active: selectedGrokFile === file.name }"
                type="button"
                @click="selectGrokFile(file.name)"
              >
                {{ file.name }}
              </button>
            </div>

            <textarea
              v-if="toolPrompt"
              v-model="toolPrompt.content"
              class="code-editor-textarea"
              spellcheck="false"
              :placeholder="toolPrompt.exists ? '在此输入提示词内容...' : '此工具尚未配置提示词。直接在此输入并保存即可创建。'"
            />

            <div class="editor-footer-bar">
              <span>
                {{
                  selectedPromptTool.usesGlobal
                    ? '当前继承公共提示词，修改保存后将自动转为专属提示词'
                    : selectedPromptTool.exists
                      ? '直接保存到工具原生配置路径'
                      : '保存后将自动创建配置文件'
                }}
              </span>
              <button
                class="btn btn-primary btn-sm"
                type="button"
                :disabled="isPromptSaving"
                @click="saveToolPrompt"
              >
                <Loader2 v-if="isPromptSaving" class="spinning" :size="12" :stroke-width="1.75" />
                <Check v-else :size="12" :stroke-width="1.75" />
                <span>{{ selectedPromptTool.usesGlobal ? '保存为专属提示词' : '保存专属提示词' }}</span>
              </button>
            </div>
          </template>
        </section>
      </div>

      <!-- View: Tool Updates -->
      <div v-else-if="activeView === 'updates'" class="workspace-scrollable">
        <section class="update-matrix-container">
          <div class="table-header-bar">
            <div style="display: flex; align-items: baseline; gap: 8px;">
              <span class="dashboard-section-title">本机已安装 CLI 工具</span>
              <span class="toolbar-subtitle">
                {{ toolUpdates.length }} 项{{ updateAvailableCount > 0 ? ` · ${updateAvailableCount} 项可升级` : '' }}
              </span>
            </div>
            <button
              v-if="updateAvailableCount > 0"
              class="btn btn-primary btn-sm"
              type="button"
              :disabled="isUpdateLoading || isBatchUpgrading || upgradingToolId !== null"
              @click="upgradeAllAvailableTools"
            >
              <Loader2 v-if="isBatchUpgrading" class="spinning" :size="12" :stroke-width="1.75" />
              <ArrowUpCircle v-else :size="12" :stroke-width="1.75" />
              <span>
                {{
                  isBatchUpgrading && batchUpgradeProgress
                    ? `正在升级 (${batchUpgradeProgress.current}/${batchUpgradeProgress.total})`
                    : `一键全部升级 (${updateAvailableCount})`
                }}
              </span>
            </button>
          </div>

          <div v-if="isUpdateLoading && !toolUpdates.length" class="skeleton-list">
            <div v-for="index in 4" :key="index" class="skeleton-row" />
          </div>

          <div v-else-if="updateError" class="empty-state-box">
            <AlertCircle :size="24" :stroke-width="1.75" />
            <h3>检查失败</h3>
            <p>{{ updateError }}</p>
            <button class="btn btn-secondary" type="button" @click="scanToolUpdates">重试</button>
          </div>

          <div v-else-if="!toolUpdates.length" class="empty-state-box">
            <ArrowUpCircle :size="24" :stroke-width="1.75" />
            <h3>尚未检查工具版本</h3>
            <p>点击右上角「检查更新」读取本机工具版本与发布信息。</p>
          </div>

          <div v-else class="table-list">
            <article v-for="tool in toolUpdates" :key="tool.id" class="update-row-item">
              <div class="row-icon-cell">
                <Terminal :size="15" :stroke-width="1.75" />
              </div>

              <div class="update-info-cell">
                <div style="display: flex; align-items: center; gap: 8px;">
                  <h3>{{ tool.name }}</h3>
                  <span v-if="tool.installMethod" class="tool-method-pill">
                    {{ tool.installMethod }}
                  </span>
                </div>
                <p>{{ tool.message }}</p>
              </div>

              <div class="update-version-cell">
                <span class="version-tag">{{ tool.currentVersion ?? '未安装' }}</span>
                <ArrowRight :size="12" :stroke-width="1.75" style="color: var(--text-tertiary);" />
                <span
                  class="version-tag"
                  :class="{ new: tool.status === 'updateAvailable' }"
                >
                  {{ tool.latestVersion ?? '未知' }}
                </span>
              </div>

              <div class="update-action-cell">
                <button
                  v-if="tool.status === 'updateAvailable'"
                  class="btn btn-primary btn-sm"
                  type="button"
                  :disabled="
                    isUpdateLoading ||
                    isBatchUpgrading ||
                    upgradingToolId !== null
                  "
                  @click="upgradeSingleTool(tool)"
                >
                  <Loader2
                    v-if="
                      upgradingToolId === tool.id ||
                      (isBatchUpgrading && batchUpgradeProgress?.toolName === tool.name)
                    "
                    class="spinning"
                    :size="12"
                    :stroke-width="1.75"
                  />
                  <ArrowUpCircle v-else :size="12" :stroke-width="1.75" />
                  <span>
                    {{
                      upgradingToolId === tool.id ||
                      (isBatchUpgrading && batchUpgradeProgress?.toolName === tool.name)
                        ? '升级中'
                        : '升级'
                    }}
                  </span>
                </button>
                <span
                  v-else
                  class="badge"
                  :class="{
                    'badge-ready': tool.status === 'latest',
                    'badge-missing': tool.status === 'notInstalled',
                    'badge-unavailable': tool.status === 'unavailable',
                  }"
                >
                  {{ updateStatusLabel(tool.status) }}
                </span>
              </div>
            </article>
          </div>
        </section>
      </div>

      <!-- View: History -->
      <div v-else-if="activeView === 'history'" class="workspace-scrollable">
        <section class="table-container">
          <div class="table-header-bar">
            <span class="dashboard-section-title">清理历史流水</span>
            <span class="toolbar-subtitle">{{ cleanupHistory.length }} 条记录</span>
          </div>

          <div v-if="isHistoryLoading && !cleanupHistory.length" class="skeleton-list">
            <div v-for="index in 4" :key="index" class="skeleton-row" />
          </div>

          <div v-else-if="historyError" class="empty-state-box">
            <AlertCircle :size="24" :stroke-width="1.75" />
            <h3>读取记录失败</h3>
            <p>{{ historyError }}</p>
            <button class="btn btn-secondary" type="button" @click="loadCleanupHistory">重试</button>
          </div>

          <div v-else-if="!cleanupHistory.length" class="empty-state-box">
            <History :size="24" :stroke-width="1.75" />
            <h3>暂无清理记录</h3>
            <p>完成一次垃圾清理后，操作审计与释放容量将在此展示。</p>
          </div>

          <div v-else class="table-list">
            <article
              v-for="entry in cleanupHistory"
              :key="`${entry.createdAt}-${entry.id}`"
              class="history-row-item"
            >
              <div
                class="history-icon-circle"
                :class="entry.status === 'success' ? 'success' : 'failed'"
              >
                <Check v-if="entry.status === 'success'" :size="12" :stroke-width="2" />
                <X v-else :size="12" :stroke-width="2" />
              </div>

              <div class="row-info-cell">
                <div class="row-name-line">
                  <h3>{{ entry.targetName }}</h3>
                  <span
                    class="badge"
                    :class="entry.status === 'success' ? 'badge-ready' : 'badge-unavailable'"
                  >
                    {{ historyStatusLabel(entry.status) }}
                  </span>
                </div>
                <p class="row-desc">{{ entry.message }}</p>
                <p v-if="entry.skippedEntries.length" class="row-blocker-note">
                  已安全保留 {{ entry.skippedEntries.length }} 个正在占用的文件
                </p>
              </div>

              <div class="history-meta-cell">
                <strong>
                  {{ entry.status === 'success' ? `释放 ${formatBytes(entry.freedBytes)}` : '未释放空间' }}
                </strong>
                <time :datetime="new Date(entry.createdAt * 1000).toISOString()">
                  {{ formatDateTime(entry.createdAt) }}
                </time>
              </div>
            </article>
          </div>
        </section>
      </div>

      <!-- View: DeepSeek Balance -->
      <div v-else-if="activeView === 'deepseek'" class="workspace-scrollable">
        <div class="deepseek-dashboard">
          <!-- Balance Summary Hero Card -->
          <section class="deepseek-hero-card">
            <div class="deepseek-hero-header">
              <div class="deepseek-hero-title-group">
                <div class="deepseek-avatar">
                  <Bot :size="20" :stroke-width="1.75" />
                </div>
                <div>
                  <h2>DeepSeek 账户额度总览</h2>
                  <p>
                    官方实时可用余额与代金券明细
                    <span v-if="deepseekBalance?.updated_at">
                      · 上次同步：{{ formatDateTime(deepseekBalance.updated_at) }}
                    </span>
                  </p>
                </div>
              </div>
              <div>
                <span
                  v-if="deepseekBalance && deepseekBalance.success"
                  class="badge"
                  :class="deepseekBalance.is_available ? 'badge-ready' : 'badge-unavailable'"
                >
                  {{ deepseekBalance.is_available ? '正常可用' : '额度不足 / 冻结' }}
                </span>
                <span v-else-if="!deepseekApiKey" class="badge badge-missing">
                  尚未配置 API Key
                </span>
                <span v-else class="badge badge-inUse">
                  待同步
                </span>
              </div>
            </div>

            <!-- Metric Box Grid -->
            <div class="deepseek-metrics-grid">
              <div class="deepseek-metric-box">
                <div class="deepseek-metric-top">
                  <span>总可用余额</span>
                  <div class="deepseek-metric-icon">
                    <Wallet :size="14" :stroke-width="1.75" />
                  </div>
                </div>
                <div class="deepseek-metric-value highlight">
                  <template v-if="deepseekBalance?.balance_infos?.[0]">
                    {{ deepseekBalance.balance_infos[0].currency === 'CNY' ? '¥' : '$' }}
                    {{ deepseekBalance.balance_infos[0].total_balance }}
                  </template>
                  <template v-else>-</template>
                </div>
                <div class="deepseek-metric-desc">
                  充值现金 + 赠送代金券总和
                </div>
              </div>

              <div class="deepseek-metric-box">
                <div class="deepseek-metric-top">
                  <span>现金充值余额</span>
                  <div class="deepseek-metric-icon">
                    <CreditCard :size="14" :stroke-width="1.75" />
                  </div>
                </div>
                <div class="deepseek-metric-value">
                  <template v-if="deepseekBalance?.balance_infos?.[0]">
                    {{ deepseekBalance.balance_infos[0].currency === 'CNY' ? '¥' : '$' }}
                    {{ deepseekBalance.balance_infos[0].topped_up_balance }}
                  </template>
                  <template v-else>-</template>
                </div>
                <div class="deepseek-metric-desc">
                  个人/企业自主充值的可用现金
                </div>
              </div>

              <div class="deepseek-metric-box">
                <div class="deepseek-metric-top">
                  <span>赠送代金券余额</span>
                  <div class="deepseek-metric-icon">
                    <Coins :size="14" :stroke-width="1.75" />
                  </div>
                </div>
                <div class="deepseek-metric-value">
                  <template v-if="deepseekBalance?.balance_infos?.[0]">
                    {{ deepseekBalance.balance_infos[0].currency === 'CNY' ? '¥' : '$' }}
                    {{ deepseekBalance.balance_infos[0].granted_balance }}
                  </template>
                  <template v-else>-</template>
                </div>
                <div class="deepseek-metric-desc">
                  系统优先抵扣赠送体验金
                </div>
              </div>
            </div>
          </section>

          <!-- Error Alert Banner -->
          <div
            v-if="deepseekBalance && !deepseekBalance.success && deepseekBalance.error_message"
            class="deepseek-alert-box error"
          >
            <AlertCircle :size="16" :stroke-width="1.75" />
            <div>
              <strong>查询出现异常：</strong>
              <span>{{ deepseekBalance.error_message }}</span>
            </div>
          </div>

          <!-- API Key Configuration Panel -->
          <section class="deepseek-panel-card">
            <div class="deepseek-panel-header">
              <h3>API Key 密钥管理</h3>
              <p>密钥将安全加密保存于本机本地存储中，仅用于与 DeepSeek 官方服务器进行认证通信。</p>
            </div>

            <div class="deepseek-input-row">
              <div class="deepseek-input-wrapper">
                <Key :size="14" :stroke-width="1.75" class="deepseek-input-icon" />
                <input
                  v-model="inputApiKey"
                  :type="showApiKey ? 'text' : 'password'"
                  class="deepseek-input"
                  placeholder="sk-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
                  autocomplete="off"
                  spellcheck="false"
                  @keydown.enter="saveDeepSeekKey"
                />
                <button
                  class="deepseek-input-toggle"
                  type="button"
                  :title="showApiKey ? '隐藏密钥' : '显示明文'"
                  @click="showApiKey = !showApiKey"
                >
                  <EyeOff v-if="showApiKey" :size="14" :stroke-width="1.75" />
                  <Eye v-else :size="14" :stroke-width="1.75" />
                </button>
              </div>

              <button
                class="btn btn-primary"
                type="button"
                :disabled="isSavingApiKey || !inputApiKey.trim()"
                @click="saveDeepSeekKey"
              >
                <Loader2 v-if="isSavingApiKey" class="spinning" :size="13" :stroke-width="1.75" />
                <Check v-else :size="13" :stroke-width="1.75" />
                <span>{{ isSavingApiKey ? '保存中...' : '保存密钥' }}</span>
              </button>

              <button
                class="btn btn-secondary"
                type="button"
                :disabled="isDeepseekLoading || !inputApiKey.trim()"
                @click="fetchDeepSeekBalance()"
              >
                <RefreshCw :class="{ spinning: isDeepseekLoading }" :size="13" :stroke-width="1.75" />
                <span>立即查询</span>
              </button>

              <button
                v-if="deepseekApiKey"
                class="btn btn-secondary"
                type="button"
                :disabled="isSavingApiKey"
                @click="clearDeepSeekKey"
              >
                <Trash2 :size="13" :stroke-width="1.75" />
                <span>清除</span>
              </button>
            </div>

            <div class="deepseek-panel-footer">
              <span>
                状态：{{
                  deepseekApiKey
                    ? isKeyFromEnv
                      ? '已自动从本机环境变量 DEEPSEEK_API_KEY 关联'
                      : '已保存本地专属配置'
                    : '未配置密钥'
                }}
              </span>
              <div class="deepseek-quick-links">
                <button
                  class="deepseek-link"
                  type="button"
                  @click="openExternalLink('https://platform.deepseek.com/api_keys')"
                >
                  <span>获取 DeepSeek API Key</span>
                  <ExternalLink :size="11" :stroke-width="1.75" />
                </button>
                <button
                  class="deepseek-link"
                  type="button"
                  @click="openExternalLink('https://platform.deepseek.com/top_up')"
                >
                  <span>前往充值</span>
                  <ExternalLink :size="11" :stroke-width="1.75" />
                </button>
              </div>
            </div>
          </section>

          <!-- Status Bar Tray Switch Card -->
          <section class="deepseek-panel-card">
            <div class="deepseek-switch-row">
              <div class="deepseek-switch-info">
                <div class="deepseek-switch-title">
                  <Monitor :size="16" :stroke-width="1.75" />
                  <strong>macOS 状态栏实时显示余额</strong>
                </div>
                <p>
                  在屏幕右上角菜单栏常驻显示 DeepSeek 可用余额（如 <code>{{ deepseekBalance?.balance_infos?.[0]?.currency === 'CNY' ? '¥' : '$' }}{{ deepseekBalance?.balance_infos?.[0]?.total_balance || '37.88' }}</code>），点击快速呼出主界面
                </p>
              </div>

              <button
                class="switch-control"
                :class="{ active: showTrayBalance }"
                type="button"
                role="switch"
                :aria-checked="showTrayBalance"
                :disabled="isTogglingTray"
                @click="toggleTraySwitch"
              >
                <span class="switch-track" />
                <span class="switch-label">{{ showTrayBalance ? '已开启' : '已关闭' }}</span>
              </button>
            </div>
          </section>

          <!-- Auto-sync Interval Settings Card -->
          <section class="deepseek-panel-card">
            <div class="deepseek-switch-row">
              <div class="deepseek-switch-info">
                <div class="deepseek-switch-title">
                  <Clock :size="16" :stroke-width="1.75" />
                  <strong>自动定时同步余额</strong>
                </div>
                <p>
                  应用在后台以低功耗定时任务自动连接 DeepSeek 官方接口，同步最新余额并刷新状态栏
                </p>
              </div>

              <div style="display: flex; align-items: center; gap: 14px;">
                <!-- Interval Segmented Control -->
                <div class="segmented-control" :style="{ opacity: autoSyncEnabled ? 1 : 0.45 }">
                  <button
                    class="segmented-option"
                    :class="{ active: autoSyncInterval === 30 }"
                    :disabled="!autoSyncEnabled || isUpdatingAutoSync"
                    type="button"
                    @click="setAutoSyncInterval(30)"
                  >
                    <span>30 分钟</span>
                  </button>
                  <button
                    class="segmented-option"
                    :class="{ active: autoSyncInterval === 60 }"
                    :disabled="!autoSyncEnabled || isUpdatingAutoSync"
                    type="button"
                    @click="setAutoSyncInterval(60)"
                  >
                    <span>1 小时</span>
                  </button>
                </div>

                <!-- On/Off Switch -->
                <button
                  class="switch-control"
                  :class="{ active: autoSyncEnabled }"
                  type="button"
                  role="switch"
                  :aria-checked="autoSyncEnabled"
                  :disabled="isUpdatingAutoSync"
                  @click="toggleAutoSync"
                >
                  <span class="switch-track" />
                  <span class="switch-label">{{ autoSyncEnabled ? '已开启' : '已关闭' }}</span>
                </button>
              </div>
            </div>
          </section>
        </div>
      </div>
    </main>

    <!-- macOS Sheet Modal: Confirm Cleanup -->
    <dialog ref="confirmDialog" class="macos-sheet" @cancel.prevent="closeConfirm">
      <div v-if="selectedItem" class="sheet-body">
        <div class="sheet-header">
          <div class="sheet-icon-box danger">
            <Trash2 :size="18" :stroke-width="1.75" />
          </div>
          <div class="sheet-copy">
            <h2>确认清理 {{ selectedItem.name }}？</h2>
            <p>
              将安全清理约 <strong>{{ formatBytes(selectedItem.sizeBytes) }}</strong> 的缓存文件。{{ selectedItem.cleanupNote }}
            </p>
          </div>
        </div>

        <div class="sheet-path-preview">
          <span v-for="path in selectedItem.paths" :key="path">{{ path }}</span>
        </div>
      </div>

      <div class="sheet-footer">
        <button
          class="btn btn-secondary"
          type="button"
          :disabled="Boolean(cleaningId)"
          @click="closeConfirm"
        >
          取消
        </button>
        <button
          class="btn btn-danger"
          type="button"
          :disabled="Boolean(cleaningId)"
          @click="cleanSelected"
        >
          <Loader2 v-if="cleaningId" class="spinning" :size="13" :stroke-width="1.75" />
          <Trash2 v-else :size="13" :stroke-width="1.75" />
          <span>{{ cleaningId ? '正在清理...' : '确认清理' }}</span>
        </button>
      </div>
    </dialog>

    <!-- macOS Sheet Modal: Enable Global Prompts -->
    <dialog ref="promptEnableDialog" class="macos-sheet" @cancel.prevent="promptEnableDialog?.close()">
      <div class="sheet-body">
        <div class="sheet-header">
          <div class="sheet-icon-box">
            <FileCode2 :size="18" :stroke-width="1.75" />
          </div>
          <div class="sheet-copy">
            <h2>开启公共提示词</h2>
            <p>
              启用后，支持共享的开发工具将统一引用同一份公共提示词。各工具现有的专属配置文件将自动备份，关闭或单独退出时可完整恢复。
            </p>
          </div>
        </div>
      </div>

      <div class="sheet-footer">
        <button
          class="btn btn-secondary"
          type="button"
          :disabled="isPromptSwitching"
          @click="promptEnableDialog?.close()"
        >
          取消
        </button>
        <button
          class="btn btn-primary"
          type="button"
          :disabled="isPromptSwitching"
          @click="setGlobalPromptEnabled(true)"
        >
          <Loader2 v-if="isPromptSwitching" class="spinning" :size="13" :stroke-width="1.75" />
          <Check v-else :size="13" :stroke-width="1.75" />
          <span>确认开启</span>
        </button>
      </div>
    </dialog>

    <!-- macOS Sheet Modal: Changelog / Release Notes -->
    <dialog ref="changelogDialog" class="macos-sheet changelog-sheet" @cancel.prevent="closeChangelog">
      <div class="sheet-body changelog-sheet-body">
        <div class="sheet-header">
          <div class="sheet-icon-box">
            <Sparkles :size="18" :stroke-width="1.75" />
          </div>
          <div class="sheet-copy">
            <div style="display: flex; align-items: center; gap: 8px;">
              <h2>DevTidy 更新日志</h2>
              <span class="changelog-version-badge">v{{ appVersion }}</span>
            </div>
            <p>集中维护本机开发缓存、AI 提示词规则与工具版本</p>
          </div>
        </div>

        <div class="changelog-timeline-list">
          <article
            v-for="entry in changelogs"
            :key="entry.version"
            class="changelog-entry-card"
            :class="{ current: entry.isLatest }"
          >
            <div class="changelog-entry-header">
              <div style="display: flex; align-items: center; gap: 6px;">
                <span class="changelog-tag">v{{ entry.version }}</span>
                <span v-if="entry.isLatest" class="changelog-current-pill">当前版本</span>
              </div>
              <time class="changelog-date">{{ entry.date }}</time>
            </div>
            <ul class="changelog-highlights-list">
              <li v-for="(item, idx) in entry.highlights" :key="idx">
                {{ item }}
              </li>
            </ul>
          </article>
        </div>
      </div>

      <div class="sheet-footer">
        <button
          class="btn btn-secondary"
          type="button"
          @click="closeChangelog"
        >
          关闭
        </button>
      </div>
    </dialog>

    <!-- Floating Toast Capsule -->
    <div v-if="toast" class="toast-capsule" :class="`toast-${toast.type}`" role="status">
      <Check v-if="toast.type === 'success'" :size="14" :stroke-width="2" />
      <AlertCircle v-else :size="14" :stroke-width="2" />
      <span>{{ toast.message }}</span>
    </div>
  </div>
</template>
