import { computed, makeObservable, observable } from "mobx";
import { ResType, streamRequest } from "@/services/Axios";
import { notification, UploadFile } from "antd";
import { deepCopy, generateRandomStr, getTimeDifference } from "hsu-utils";
import dayjs from "dayjs";
import {
  AnswerMessage,
  ChatHistoryData,
  ChatMessage,
  RetrieverResource,
} from "@hsu-react/ui";
import { routerHistory } from "@hsu-react/single-router";

// 常量定义
const NEW_CHAT_ID = "NEW_CHAT";
const END_THINK_FLAG = "[ENDTHINKFLAG]";
const REDACTED_REASONING_START = "<think>";
const REDACTED_REASONING_END = "</think>";
const HISTORY_GROUP_TODAY = "今天";
const HISTORY_GROUP_YESTERDAY = "昨天";
const HISTORY_GROUP_WITHIN_7_DAYS = "7 天内";
const HISTORY_GROUP_WITHIN_30_DAYS = "30 天内";
const DEFAULT_PAGE_SIZE = 100;
const DEFAULT_PAGE = 1;

export interface AgentConfig {
  key: string;
  open: boolean;
}

interface ChatConfig {
  model: string;
  agents: AgentConfig[];
  hasThinking?: boolean;
}

interface StreamMessageData {
  conversationId?: string;
  taskId?: string;
  messageId?: string;
  event: string;
  answer?: string;
  metadata?: {
    retrieverResources?: RetrieverResource[];
  };
  message?: string;
}

export interface ChatDetailData {
  id: string;
  conversation_id: string;
  parent_message_id: null;
  inputs: Record<string, unknown>;
  query: string;
  answer: string;
  feedback: {
    rating?: "like" | "dislike";
  };
  retrieverResources: RetrieverResource[];
  created_at: number;
  messageFiles: {
    id: string;
    nm: string;
    filename: string;
    type: string;
    url: string;
    mime_type: string;
    size: number;
    transfer_method: string;
    belongs_to: string;
  }[];
  status: string;
  error: null;
  messageId: string;
  conversationId: string;
  crtTm: string;
  queryTm: string;
}

export interface ChatListData {
  id: string;
  name: string;
  inputs: Record<string, unknown>;
  status: string;
  crtTm: number;
  updTm: number;
  sortTm: number;
  ago: number;
  conversationId: string;
}

/**
 * 聊天基础 Store 类
 */
class ChatStore {
  // 流式请求接口地址
  protected accessor _streamApiUrl = "";

  // 消息列表
  @computed
  get messages() {
    return this._messages;
  }
  @observable
  protected accessor _messages = new Map<string, ChatMessage[]>();

  // 是否正在对话
  @computed
  get assistanting() {
    return this._assistanting;
  }
  @observable
  protected accessor _assistanting = new Map<string, boolean>();

  // 文件列表
  @computed
  get fileList() {
    return this._fileList;
  }
  @observable
  protected accessor _fileList = new Map<string, UploadFile[]>();

  // 当前模型
  @computed
  get currentModel() {
    return this._currentModel;
  }
  @observable
  protected accessor _currentModel = new Map<string, string>();

  // 功能键状态
  @computed
  get agentsState() {
    return this._agentsState;
  }
  @observable
  protected accessor _agentsState = new Map<string, AgentConfig[]>();

  // 历史会话列表
  @computed
  get historyList() {
    return this._historyList;
  }
  @observable
  protected accessor _historyList: Record<string, ChatHistoryData[]> = {};

  // 当前应用key
  @computed
  get currentApiKey() {
    return this._currentApiKey;
  }
  @observable
  protected accessor _currentApiKey = "";

  // 当前对话ID
  @computed
  get currentChatId() {
    return this._currentChatId;
  }
  @observable
  protected accessor _currentChatId = NEW_CHAT_ID;

  @observable
  protected accessor _abortController = new Map<
    string,
    AbortController | null
  >();
  @observable
  protected accessor _taskId = new Map<string, string>();

  // 搜索数据
  @computed
  get historySearchData() {
    return this._historySearchData;
  }
  @observable
  protected accessor _historySearchData: Record<string, string | number> = {};

  constructor() {
    makeObservable(this);
  }

  /**
   * 清理指定 chatId 的所有数据
   */
  protected _clearChatData = (chatId: string) => {
    this._messages.delete(chatId);
    this._assistanting.delete(chatId);
    this._fileList.delete(chatId);
    this._abortController.delete(chatId);
    this._taskId.delete(chatId);
    this._currentModel.delete(chatId);
    this._agentsState.delete(chatId);
  };

  /**
   * 创建新对话
   */
  public newChat = () => {
    this._currentChatId = NEW_CHAT_ID;
    this._clearChatData(NEW_CHAT_ID);
  };

  /**
   * 设置当前模型
   */
  public setCurrentModel = (model: string) => {
    this._currentModel.set(this._currentChatId, model);
  };

  /**
   * 设置功能键状态
   */
  public setAgentsState = (agents: AgentConfig[]) => {
    this._agentsState.set(this._currentChatId, agents);
  };

  /**
   * 设置当前对话ID
   */
  public setCurrentChatId = (chatId: string) => {
    if (this._currentChatId !== chatId) {
      this._currentChatId = chatId;
    }
  };

  /**
   * 设置文件列表
   */
  public setFileList = (fileList: UploadFile[]) => {
    this._fileList.set(this._currentChatId, fileList);
  };

  /**
   * 设置当前应用key
   */
  public setCurrentApiKey = (apiKey: string) => {
    this._currentApiKey = apiKey;
  };

  /**
   * 获取聊天配置
   */
  protected _getChatConfig = (
    chatId: string,
    model?: string,
    agentsState?: AgentConfig[],
    hasThinking?: boolean
  ): ChatConfig => {
    return {
      model: this._currentModel.get(chatId) || model || "",
      agents: this._agentsState.get(chatId) || agentsState || [],
      hasThinking,
    };
  };

  /**
   * 处理思考内容
   */
  protected _handleThinkContent = (
    answer: AnswerMessage,
    data: StreamMessageData,
    thinkStart: Date | null
  ) => {
    if (!data.answer) return;

    let processedAnswer = data.answer;
    if (processedAnswer === END_THINK_FLAG) {
      processedAnswer = REDACTED_REASONING_END;
    }

    if (processedAnswer === REDACTED_REASONING_END && thinkStart) {
      const { seconds, milliseconds } = getTimeDifference(
        thinkStart.toISOString(),
        new Date().toISOString()
      );
      answer.think_time = seconds + Number((milliseconds / 1000).toFixed(2));
    }

    if (answer.think === "") {
      answer.think = processedAnswer.startsWith(REDACTED_REASONING_START)
        ? processedAnswer
        : `${REDACTED_REASONING_START}${processedAnswer}`;
    } else {
      if (processedAnswer.includes(REDACTED_REASONING_END)) {
        answer.think +=
          processedAnswer.split(REDACTED_REASONING_END)[0] +
          REDACTED_REASONING_END;
        answer.answer = processedAnswer.split(REDACTED_REASONING_END)[1];
      } else {
        answer.think += processedAnswer;
      }
    }
  };

  /**
   * 处理新对话ID的迁移
   */
  protected _handleNewChatMigration = (
    newChatId: string,
    config: ChatConfig
  ) => {
    this._clearChatData(NEW_CHAT_ID);
    this._currentChatId = newChatId;
    this._assistanting.set(newChatId, true);
    this._currentModel.set(newChatId, config.model);
    this._agentsState.set(newChatId, config.agents);
    this.getHistoryList();
    routerHistory.push(`/${newChatId}`);
  };

  /**
   * 处理流消息
   */
  protected _handleStreamMessage = (
    data: StreamMessageData,
    currentChatId: string,
    chat: ChatMessage[],
    config: ChatConfig,
    thinkStart: Date | null,
    abortController: AbortController
  ): string => {
    let chatId = currentChatId;

    // 处理新对话ID迁移
    if (chatId === NEW_CHAT_ID && data.conversationId) {
      this._handleNewChatMigration(data.conversationId, config);
      chatId = data.conversationId;
    }

    // 设置任务ID
    if (data.taskId) {
      this._taskId.set(chatId, data.taskId);
    }

    // 设置新的流请求
    if (!this._abortController.has(chatId)) {
      this._abortController.set(chatId, abortController);
    }

    // 获取当前对话
    const currentChat = chat.length
      ? chat
      : [...(this._messages.get(chatId) || [])];

    // 更新消息ID
    const query = currentChat[currentChat.length - 1]?.query;
    if (query && data.messageId) {
      query.messageId = data.messageId;
    }

    // 处理回答
    const answer = currentChat[currentChat.length - 1]?.answers[0];
    if (
      answer &&
      (data.event === "agent_message" ||
        data.event === "agent_thought" ||
        data.event === "message")
    ) {
      if (data.messageId) {
        answer.messageId = data.messageId;
      }

      // 只有当模型支持思考功能时才处理思考内容
      const shouldHandleThinking = config.hasThinking !== false;

      if (data.event === "agent_thought") {
        if (shouldHandleThinking) {
          this._handleThinkContent(answer, data, thinkStart);
        }
      }

      if (data.event === "agent_message" && data.answer) {
        answer.answer += data.answer;
      }

      if (data.event === "message") {
        if (
          shouldHandleThinking &&
          !answer.think?.endsWith(REDACTED_REASONING_END)
        ) {
          this._handleThinkContent(answer, data, thinkStart);
        } else {
          if (answer.answer) {
            answer.answer += data.answer;
          } else {
            answer.answer = data.answer;
          }
        }
      }
    }

    // 结束对话
    if (data.event === "message_end") {
      if (answer) {
        if (data.metadata) {
          answer.retrieverResources = data.metadata.retrieverResources;
        }

        answer.time = dayjs().format("YYYY-MM-DD HH:mm:ss");
      }

      this._assistanting.set(chatId, false);
    }

    // 错误处理
    if (data.event.includes("error")) {
      if (answer) {
        answer.error = data.message || "";
      }

      this._assistanting.set(chatId, false);
      this._abortController.delete(chatId);
    }

    // 更新对话
    this._messages.set(chatId, currentChat);

    return chatId;
  };

  /**
   * 创建新的消息对象
   */
  protected _createNewMessage = (
    content: string,
    files: UploadFile[],
    messageId: string
  ): ChatMessage => {
    return {
      query: {
        query: content,
        messageFiles: files?.map((file) => ({
          filename: file.name,
          url: file.url || "",
          id: file.response?.id || "",
        })),
        messageId,
        time: dayjs().format("YYYY-MM-DD HH:mm:ss"),
      },
      answers: [
        {
          think_time: 0,
          think: "",
          answer: "",
          retrieverResources: [],
          messageId,
          like: null,
          time: dayjs().format("YYYY-MM-DD HH:mm:ss"),
        },
      ],
    };
  };

  /**
   * 格式化文件数据用于API请求
   */
  protected _formatFilesForApi = (files: UploadFile[]) => {
    return files.length
      ? files?.map((file) => ({
          type: "document",
          transferMethod: "local_file",
          uploadFileId: file.response?.id || "",
        }))
      : undefined;
  };

  /**
   * 处理流请求错误和关闭
   */
  protected _handleStreamEnd = (chatId: string) => {
    this._assistanting.set(chatId, false);
    this._abortController.delete(chatId);
    this.getHistoryList();
  };

  /**
   * 开始对话
   */
  public chat = ({
    apiKey,
    content,
    again,
    model,
    agentsState,
    hasThinking,
  }: {
    apiKey: string;
    content: string;
    again?: boolean;
    model?: string;
    agentsState?: AgentConfig[];
    hasThinking?: boolean;
  }) => {
    this._currentApiKey = apiKey;

    const files = deepCopy(this._fileList.get(this._currentChatId) || []);
    this._fileList.delete(this._currentChatId);

    let currentChatId = this._currentChatId;
    let thinkStart: Date | null = null;

    const config = this._getChatConfig(
      currentChatId,
      model,
      agentsState,
      hasThinking
    );
    const messageId = generateRandomStr(16);

    // 获取当前对话
    let chat = [...(this._messages.get(currentChatId) || [])];

    // 如果是重新发送，移除最后一条消息
    if (again && chat.length > 1) {
      chat = chat.slice(0, chat.length - 1);
    }

    // 添加新消息（如果不是重新发送）
    if (!again) {
      chat.push(this._createNewMessage(content, files, messageId));
    }

    // 设置对话和状态
    this._messages.set(currentChatId, chat);
    this._assistanting.set(currentChatId, true);

    // 发起流请求
    const abortController = streamRequest(this._streamApiUrl, {
      data: {
        content,
        apiKey,
        files: this._formatFilesForApi(files),
        conversationId: currentChatId === NEW_CHAT_ID ? "" : currentChatId,
        inputs: {
          ...config.agents.reduce((acc, item) => {
            acc[item.key as keyof typeof acc] = `${+item.open}`;
            return acc;
          }, {} as Record<string, string>),
          model: config.model,
        },
      },
      onopen: async () => {
        thinkStart = new Date();
      },
      onmessage: (messages) => {
        try {
          const data: StreamMessageData = JSON.parse(messages.data);
          currentChatId = this._handleStreamMessage(
            data,
            currentChatId,
            chat,
            config,
            thinkStart,
            abortController
          );
        } catch (error) {
          // 设置错误信息到回答
          const currentChat = this._messages.get(currentChatId) || chat;
          const answer = currentChat[currentChat.length - 1]?.answers[0];
          if (answer) {
            answer.error =
              error instanceof Error ? error.message : String(error);
            this._messages.set(currentChatId, currentChat);
          }
          this._handleStreamEnd(currentChatId);
        }
      },
      onerror: (error) => {
        // 设置错误信息到回答
        const currentChat = this._messages.get(currentChatId) || chat;
        const answer = currentChat[currentChat.length - 1]?.answers[0];
        if (answer) {
          answer.error = error instanceof Error ? error.message : String(error);
          this._messages.set(currentChatId, currentChat);
        }
        this._handleStreamEnd(currentChatId);
      },
      onclose: () => {
        this._handleStreamEnd(currentChatId);
      },
    });
  };

  /**
   * 停止对话
   */
  public stop = () => {
    const chatId = this._currentChatId;
    const taskId = this._taskId.get(chatId);
    const abortController = this._abortController.get(chatId);

    if (taskId && abortController) {
      this._stopChat({
        taskId,
        apiKey: this._currentApiKey,
      });
    }

    abortController?.abort();
    this._abortController.delete(chatId);
    this._assistanting.set(chatId, false);
    this.getHistoryList();
  };

  /**
   * 停止对话 API（子类需要实现）
   */
  protected _stopChat = (_params: { taskId: string; apiKey: string }) => {
    void _params;
  };

  /**
   * 解析答案内容，分离思考部分和回答部分
   */
  protected _parseAnswerContent = (answer: string) => {
    const processedAnswer = answer.replace(
      END_THINK_FLAG,
      REDACTED_REASONING_END
    );

    if (processedAnswer.includes(REDACTED_REASONING_END)) {
      let think = processedAnswer.split(REDACTED_REASONING_END)[0];
      if (!think.startsWith(REDACTED_REASONING_START)) {
        think = `${REDACTED_REASONING_START}${think}`;
      }
      think += REDACTED_REASONING_END;

      const answerText = processedAnswer.split(REDACTED_REASONING_END)[1] || "";
      return { think, answer: answerText };
    }

    return { think: "", answer: processedAnswer };
  };

  /**
   * 转换对话详情数据为消息格式
   */
  protected _transformChatDetailToMessage = (
    item: ChatDetailData
  ): ChatMessage => {
    const { think, answer } = this._parseAnswerContent(item.answer);
    const like =
      item.feedback?.rating === "like"
        ? true
        : item.feedback?.rating === "dislike"
        ? false
        : null;

    return {
      query: {
        query: item.query,
        messageFiles: (item?.messageFiles || [])?.map((file) => ({
          filename: window.decodeURIComponent(file.filename || file.nm),
          url: file.url?.startsWith("http") ? file.url : "",
          id: file.id,
        })),
        messageId: item.messageId,
        time: item.queryTm,
      },
      answers: [
        {
          answer,
          think_time: 0,
          think,
          retrieverResources: item.retrieverResources || [],
          messageId: item.messageId,
          like,
          time: item.queryTm,
        },
      ],
    };
  };

  /**
   * 获取对话详情
   */
  public getChatDetail = (chatId: string, apiKey?: string) => {
    if (this._currentChatId !== chatId) {
      this._currentChatId = chatId;
    }

    if (this._assistanting.get(chatId) === true) {
      return;
    }

    this._getChatDetailApi({
      apiKey: apiKey || this._currentApiKey,
      conversationId: chatId,
      page: DEFAULT_PAGE,
      pageSize: DEFAULT_PAGE_SIZE,
    })
      .then((res) => {
        const { list } = res.data;

        const messages: ChatMessage[] =
          list?.map((item) => this._transformChatDetailToMessage(item)) || [];

        // 保存配置信息
        const lastItem = list?.[list.length - 1];
        if (lastItem?.inputs) {
          this._currentModel.set(chatId, lastItem.inputs.model as string);
          const agents: AgentConfig[] = Object.keys(lastItem.inputs)?.map(
            (key) => ({
              key,
              open:
                lastItem.inputs[key as keyof typeof lastItem.inputs] === "1",
            })
          );
          this._agentsState.set(chatId, agents);
        }

        this._messages.set(chatId, messages);
      })
      .catch(() => {
        this._message({ code: 500, msg: "获取对话详情失败" } as ResType);
      });
  };

  /**
   * 获取对话详情 API（子类需要实现）
   */
  protected _getChatDetailApi = (_params: {
    apiKey: string;
    conversationId: string;
    page: number;
    pageSize: number;
  }): Promise<{ data: { list: ChatDetailData[] } }> => {
    void _params;
    return Promise.resolve({ data: { list: [] } });
  };

  /**
   * 消息反馈
   */
  public messageFeedback = (
    messageId: string,
    rating: boolean | null,
    content: string
  ) => {
    this._messageFeedbackApi({
      apiKey: this._currentApiKey,
      messageId,
      rating: rating === null ? "UNLIKE" : rating ? "LIKE" : "DISLIKE",
      content,
    });
  };

  /**
   * 消息反馈 API（子类需要实现）
   */
  protected _messageFeedbackApi = (_params: {
    apiKey: string;
    messageId: string;
    rating: "LIKE" | "DISLIKE" | "UNLIKE";
    content: string;
  }) => {
    void _params;
  };

  /**
   * 清空历史会话列表
   */
  public clearHistoryList = () => {
    this._historyList = {};
  };

  /**
   * 获取对话列表
   */
  public getHistoryList = async (
    searchData?: Record<string, string | number>
  ) => {
    if (searchData) {
      this._historySearchData = searchData;
    }

    try {
      const res = await this._getChatListApi({
        apiKey: this._currentApiKey,
        page: DEFAULT_PAGE,
        pageSize: DEFAULT_PAGE_SIZE,
      });

      const { list } = res.data;
      this._historyList = this._formatHistoryList(list);
    } catch {
      this._message({ code: 500, msg: "获取对话列表失败" } as ResType);
    }
  };

  /**
   * 获取对话列表 API（子类需要实现）
   */
  protected _getChatListApi = (_params: {
    apiKey: string;
    page: number;
    pageSize: number;
  }): Promise<{ data: { list: ChatListData[] } }> => {
    void _params;
    return Promise.resolve({ data: { list: [] } });
  };

  /**
   * 获取历史记录的分组键
   */
  protected _getHistoryGroupKey = (ago: number): string => {
    if (ago > 30) {
      return dayjs().format("YYYY-MM");
    }
    if (ago === 0) {
      return HISTORY_GROUP_TODAY;
    }
    if (ago === 1) {
      return HISTORY_GROUP_YESTERDAY;
    }
    if (ago <= 7) {
      return HISTORY_GROUP_WITHIN_7_DAYS;
    }
    return HISTORY_GROUP_WITHIN_30_DAYS;
  };

  /**
   * 添加历史记录到分组
   */
  protected _addToHistoryGroup = (
    groups: Record<string, ChatHistoryData[]>,
    item: ChatHistoryData,
    groupKey: string
  ) => {
    if (!groups[groupKey]) {
      groups[groupKey] = [];
    }
    groups[groupKey].push(item);
  };

  /**
   * 格式化历史会话列表
   */
  protected _formatHistoryList = (
    data: ChatListData[]
  ): Record<string, ChatHistoryData[]> => {
    // 添加时间差信息并排序
    const historyList: ChatHistoryData[] = data?.map((item) => ({
      ...item,
      ago: getTimeDifference(
        dayjs(item.sortTm).format("YYYY-MM-DD"),
        dayjs().format("YYYY-MM-DD")
      ).days,
    }));

    // 按时间分组
    const groupedHistory: Record<string, ChatHistoryData[]> = {};

    historyList?.forEach((item) => {
      const ago = item.ago as number;
      let groupKey: string;

      if (ago > 30) {
        // 超过30天的按月份分组
        groupKey = dayjs(item.updTm as number).format("YYYY-MM");
      } else {
        groupKey = this._getHistoryGroupKey(ago);
      }

      this._addToHistoryGroup(groupedHistory, item, groupKey);
    });

    return groupedHistory;
  };

  /**
   * 更新对话标题
   */
  public updateTitle = (chatId: string, title: string) => {
    this._updateTitleApi({
      apiKey: this._currentApiKey,
      conversationId: chatId,
      name: title,
    }).catch(() => {
      this._message({ code: 500, msg: "更新对话标题失败" } as ResType);
    });
  };

  /**
   * 更新对话标题 API（子类需要实现）
   */
  protected _updateTitleApi = (_params: {
    apiKey: string;
    conversationId: string;
    name: string;
  }): Promise<void> => {
    void _params;
    return Promise.resolve();
  };

  /**
   * 删除历史会话
   */
  public deleteHistory = (chatId: string, fn?: () => void) => {
    this._deleteChatApi({
      apiKey: this._currentApiKey,
      conversationId: chatId,
    })
      .then(() => {
        this.getHistoryList();
        fn?.();
      })
      .catch(() => {
        this._message({ code: 500, msg: "删除历史会话失败" } as ResType);
      });
  };

  /**
   * 删除历史会话 API（子类需要实现）
   */
  protected _deleteChatApi = (_params: {
    apiKey: string;
    conversationId: string;
  }): Promise<void> => {
    void _params;
    return Promise.resolve();
  };

  /**
   * 消息处理
   */
  protected _message = (res?: ResType) => {
    if (res?.code === 200) {
      notification.success({
        message: res?.msg || "成功",
      });
    } else {
      notification.error({
        message: res?.msg || "失败",
      });
    }
  };
}

export default ChatStore;
