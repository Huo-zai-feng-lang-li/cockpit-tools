import { create } from 'zustand';
import {
  CodexAccount,
  CodexHotSwitchResponse,
  CodexQuota,
  hasCodexAccountStructure,
  hasCodexAccountName,
  isCodexTeamLikePlan,
} from '../types/codex';
import * as codexService from '../services/codexService';
import { emitAccountsChanged, emitCurrentAccountChanged } from '../utils/accountSyncEvents';
import { message as showMessage } from '@tauri-apps/plugin-dialog';

const CODEX_ACCOUNTS_CACHE_KEY = 'agtools.codex.accounts.cache';
const CODEX_CURRENT_ACCOUNT_CACHE_KEY = 'agtools.codex.accounts.current';
const CODEX_PROFILE_SYNC_IN_FLIGHT = new Set<string>();
const CODEX_PROFILE_SYNC_LAST_ATTEMPT = new Map<string, number>();
const CODEX_PROFILE_SYNC_RETRY_INTERVAL_MS = 5 * 60 * 1000;
let codexCurrentAccountEpoch = 0;

const loadCachedCodexAccounts = () => {
  try {
    const raw = localStorage.getItem(CODEX_ACCOUNTS_CACHE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
};

const loadCachedCodexCurrentAccount = () => {
  try {
    const raw = localStorage.getItem(CODEX_CURRENT_ACCOUNT_CACHE_KEY);
    if (!raw) return null;
    return JSON.parse(raw) as CodexAccount;
  } catch {
    return null;
  }
};

const persistCodexAccountsCache = (accounts: CodexAccount[]) => {
  try {
    localStorage.setItem(CODEX_ACCOUNTS_CACHE_KEY, JSON.stringify(accounts));
  } catch {
    // ignore cache write failures
  }
};

const persistCodexCurrentAccountCache = (account: CodexAccount | null) => {
  try {
    if (!account) {
      localStorage.removeItem(CODEX_CURRENT_ACCOUNT_CACHE_KEY);
      return;
    }
    localStorage.setItem(CODEX_CURRENT_ACCOUNT_CACHE_KEY, JSON.stringify(account));
  } catch {
    // ignore cache write failures
  }
};

const shouldHydrateCodexProfile = (account: CodexAccount): boolean =>
  !hasCodexAccountStructure(account) ||
  (isCodexTeamLikePlan(account.plan_type) && !hasCodexAccountName(account));

interface CodexAccountState {
  accounts: CodexAccount[];
  currentAccount: CodexAccount | null;
  loading: boolean;
  error: string | null;
  
  // Actions
  fetchAccounts: () => Promise<void>;
  fetchCurrentAccount: () => Promise<void>;
  switchAccount: (accountId: string) => Promise<CodexAccount>;
  hotSwitchAccount: (accountId: string) => Promise<CodexHotSwitchResponse>;
  deleteAccount: (accountId: string) => Promise<void>;
  deleteAccounts: (accountIds: string[]) => Promise<void>;
  refreshQuota: (accountId: string) => Promise<CodexQuota>;
  refreshAllQuotas: () => Promise<number>;
  refreshAllQuotasExceptCurrent: () => Promise<number>;
  hydrateAccountProfilesIfNeeded: (accountIds?: string[]) => Promise<void>;
  importFromLocal: () => Promise<CodexAccount>;
  importFromJson: (jsonContent: string) => Promise<CodexAccount[]>;
  updateAccountName: (accountId: string, name: string) => Promise<CodexAccount>;
  updateApiKeyCredentials: (
    accountId: string,
    apiKey: string,
    apiBaseUrl?: string,
  ) => Promise<CodexAccount>;
  updateAccountTags: (accountId: string, tags: string[]) => Promise<CodexAccount>;
}

export const useCodexAccountStore = create<CodexAccountState>((set, get) => ({
  accounts: loadCachedCodexAccounts(),
  currentAccount: loadCachedCodexCurrentAccount(),
  loading: false,
  error: null,
  
  fetchAccounts: async () => {
    set({ loading: true, error: null });
    try {
      const accounts = await codexService.listCodexAccounts();
      set({ accounts, loading: false });
      persistCodexAccountsCache(accounts);
      void get().hydrateAccountProfilesIfNeeded(accounts.map((account) => account.id));
    } catch (e) {
      set({ error: String(e), loading: false });
    }
  },
  
  fetchCurrentAccount: async () => {
    const requestEpoch = codexCurrentAccountEpoch;
    try {
      const currentAccount = await codexService.getCurrentCodexAccount();
      if (requestEpoch !== codexCurrentAccountEpoch) return;
      set({ currentAccount });
      persistCodexCurrentAccountCache(currentAccount);
    } catch (e) {
      console.error('获取当前 Codex 账号失败:', e);
    }
  },
  
  hotSwitchAccount: async (accountId: string) => {
    codexCurrentAccountEpoch += 1;
    const response = await codexService.hotSwitchCodexAccount(accountId);
    const account = response.account;
    codexCurrentAccountEpoch += 1;
    set((state) => {
      const nextAccounts = state.accounts.map((item) =>
        item.id === account.id ? { ...item, ...account } : item,
      );
      persistCodexAccountsCache(nextAccounts);
      if (!response.hot_switch_error) {
        persistCodexCurrentAccountCache(account);
        return { accounts: nextAccounts, currentAccount: account };
      }
      return { accounts: nextAccounts };
    });
    await get().fetchAccounts();

    if (response.hot_switch_error) {
      console.warn('[Codex HotSwitch] CDP channel failed, showing degraded notification:', response.hot_switch_error);
      try {
        const messageText = response.shortcut_injected
          ? '未检测到 IDE 调试端口，无感切换受限。\n\n凭证已成功保存！我们已自动为您在桌面的 Antigravity 快捷方式中注入了调试端口配置，请直接通过该快捷方式重启 IDE，以后即可永久享受 100ms 极速无感热切！'
          : '未检测到 IDE 调试端口或连接受限，无感切换受限。\n\n凭证已成功保存，请重启 Antigravity IDE 以使新账号生效！';
        
        await showMessage(messageText, { title: '切号提示', kind: 'warning' });
      } catch (err) {
        console.error('Failed to show message dialog:', err);
      }
    } else {
      set({ currentAccount: account });
      persistCodexCurrentAccountCache(account);
      await emitCurrentAccountChanged({
        platformId: 'codex',
        accountId: account.id,
        reason: 'hot_switch',
      });
    }

    return response;
  },

  switchAccount: async (accountId: string) => {
    codexCurrentAccountEpoch += 1;
    const account = await codexService.switchCodexAccount(accountId);
    codexCurrentAccountEpoch += 1;
    set((state) => {
      const nextAccounts = state.accounts.map((item) =>
        item.id === account.id ? { ...item, ...account } : item,
      );
      persistCodexAccountsCache(nextAccounts);
      persistCodexCurrentAccountCache(account);
      return { accounts: nextAccounts, currentAccount: account };
    });
    await get().fetchAccounts();
    set({ currentAccount: account });
    persistCodexCurrentAccountCache(account);
    await emitCurrentAccountChanged({
      platformId: 'codex',
      accountId: account.id,
      reason: 'switch',
    });
    return account;
  },
  
  deleteAccount: async (accountId: string) => {
    const previousCurrentAccountId = get().currentAccount?.id ?? null;
    await codexService.deleteCodexAccount(accountId);
    await get().fetchAccounts();
    await get().fetchCurrentAccount();
    await emitAccountsChanged({
      platformId: 'codex',
      reason: 'delete',
    });
    const nextCurrentAccountId = get().currentAccount?.id ?? null;
    if (previousCurrentAccountId !== nextCurrentAccountId) {
      await emitCurrentAccountChanged({
        platformId: 'codex',
        accountId: nextCurrentAccountId,
        reason: 'delete',
      });
    }
  },
  
  deleteAccounts: async (accountIds: string[]) => {
    const previousCurrentAccountId = get().currentAccount?.id ?? null;
    await codexService.deleteCodexAccounts(accountIds);
    await get().fetchAccounts();
    await get().fetchCurrentAccount();
    await emitAccountsChanged({
      platformId: 'codex',
      reason: 'delete',
    });
    const nextCurrentAccountId = get().currentAccount?.id ?? null;
    if (previousCurrentAccountId !== nextCurrentAccountId) {
      await emitCurrentAccountChanged({
        platformId: 'codex',
        accountId: nextCurrentAccountId,
        reason: 'delete',
      });
    }
  },
  
  refreshQuota: async (accountId: string) => {
    const quota = await codexService.refreshCodexQuota(accountId);
    await get().fetchAccounts();
    await get().fetchCurrentAccount();
    return quota;
  },
  
  refreshAllQuotas: async () => {
    const successCount = await codexService.refreshAllCodexQuotas();
    await get().fetchAccounts();
    await get().fetchCurrentAccount();
    return successCount;
  },

  refreshAllQuotasExceptCurrent: async () => {
    const successCount = await codexService.refreshAllCodexQuotasExceptCurrent();
    await get().fetchAccounts();
    await get().fetchCurrentAccount();
    return successCount;
  },

  hydrateAccountProfilesIfNeeded: async (accountIds?: string[]) => {
    const now = Date.now();
    const scope = accountIds ? new Set(accountIds) : null;
    const candidates = get().accounts.filter(
      (account) =>
        (!scope || scope.has(account.id)) &&
        shouldHydrateCodexProfile(account) &&
        !CODEX_PROFILE_SYNC_IN_FLIGHT.has(account.id) &&
        now - (CODEX_PROFILE_SYNC_LAST_ATTEMPT.get(account.id) ?? 0) >=
          CODEX_PROFILE_SYNC_RETRY_INTERVAL_MS,
    );

    for (const account of candidates) {
      CODEX_PROFILE_SYNC_IN_FLIGHT.add(account.id);
      CODEX_PROFILE_SYNC_LAST_ATTEMPT.set(account.id, now);
      try {
        const updatedAccount = await codexService.refreshCodexAccountProfile(account.id);
        set((state) => {
          const nextAccounts = state.accounts.map((item) =>
            item.id === updatedAccount.id ? { ...item, ...updatedAccount } : item,
          );
          const nextCurrentAccount =
            state.currentAccount?.id === updatedAccount.id
              ? { ...state.currentAccount, ...updatedAccount }
              : state.currentAccount;

          persistCodexAccountsCache(nextAccounts);
          persistCodexCurrentAccountCache(nextCurrentAccount);

          return {
            accounts: nextAccounts,
            currentAccount: nextCurrentAccount,
          };
        });
      } catch (e) {
        console.warn('刷新 Codex 账号资料失败:', account.id, e);
      } finally {
        CODEX_PROFILE_SYNC_IN_FLIGHT.delete(account.id);
      }
    }
  },
  
  importFromLocal: async () => {
    const account = await codexService.importCodexFromLocal();
    await get().fetchAccounts();
    await emitAccountsChanged({
      platformId: 'codex',
      reason: 'import',
    });
    return account;
  },
  
  importFromJson: async (jsonContent: string) => {
    const accounts = await codexService.importCodexFromJson(jsonContent);
    await get().fetchAccounts();
    await emitAccountsChanged({
      platformId: 'codex',
      reason: 'import',
    });
    return accounts;
  },

  updateAccountName: async (accountId: string, name: string) => {
    const account = await codexService.updateCodexAccountName(accountId, name);
    await get().fetchAccounts();
    await get().fetchCurrentAccount();
    return account;
  },

  updateApiKeyCredentials: async (accountId: string, apiKey: string, apiBaseUrl?: string) => {
    const account = await codexService.updateCodexApiKeyCredentials(
      accountId,
      apiKey,
      apiBaseUrl,
    );
    await get().fetchAccounts();
    await get().fetchCurrentAccount();
    return account;
  },

  updateAccountTags: async (accountId: string, tags: string[]) => {
    const account = await codexService.updateCodexAccountTags(accountId, tags);
    await get().fetchAccounts();
    return account;
  },
}));
