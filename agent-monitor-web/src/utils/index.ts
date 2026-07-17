/**
 * 获取 uuid（密码学随机；用于验证码 key 等不可被预测的场景）
 */
export const getUUID = () => window.crypto.randomUUID();
