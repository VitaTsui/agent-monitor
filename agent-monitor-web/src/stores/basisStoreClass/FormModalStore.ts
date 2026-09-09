import { ResType } from "@/services/Axios";
import { message, notification } from "@hsu-react/ui";
import { computed, makeObservable, observable } from "mobx";

/**
 * F: 表单数据类型
 */
class FormModalStore<F = Record<string, unknown>> {
  constructor() {
    makeObservable(this);
  }

  /**
   * 表单数据
   */
  @computed
  get formData(): Partial<F> {
    return this._formData;
  }
  @observable
  protected accessor _formData: Partial<F> = {};

  /**
   * 表单类型
   */
  @computed
  get formType(): string {
    return this._formType;
  }
  @observable
  protected accessor _formType: string = "def";
  public setFormType = (formType: string) => {
    this._formType = formType;
  };

  /** 详情请求序号：resetFormData 后旧的延迟回调直接失效，防止脏回填 */
  private _formSeq = 0;

  /**
   * 获取详情
   * @param id
   */
  public getFormData = (id: number | string, data?: Partial<F>) => {
    const seq = ++this._formSeq;
    setTimeout(() => {
      if (seq !== this._formSeq) {
        return;
      }
      this._getFormData(id, data);
    }, 500);
  };
  protected _getFormData = (_id: number | string, _data?: Partial<F>) => {
    // 由子类实现具体的获取逻辑
  };

  /**
   * 编辑
   * @param data
   */
  public editFormData = (
    _id: number | string,
    _data: Partial<F>,
    fn?: () => void
  ) => {
    // 由子类实现具体的编辑逻辑
    fn?.();
  };

  /**
   * 新增
   * @param data
   */
  public addFormData = (_data: Partial<F>, fn?: () => void) => {
    // 由子类实现具体的新增逻辑
    fn?.();
  };

  /**
   * 重置表单数据（并使未落地的详情延迟回调失效）
   */
  public resetFormData = () => {
    this._formSeq++;
    this._formData = {};
  };

  /**
   * 消息处理
   * @param res
   */
  protected _message = (res?: ResType) => {
    if (res?.code === 0) {
      if (typeof res?.data === "string") {
        notification.success({
          title: res.data,
        });
      } else {
        message.success(res?.msg ?? "成功");
      }
    } else {
      notification.error({
        title: res?.msg ?? "失败",
      });
    }
  };

  /**
   * 重置Store
   */
  public resetStore = () => {
    this._formData = {};

    this._resetStore();
  };

  protected _resetStore = () => {};
}

export default FormModalStore;
