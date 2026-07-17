import React, { useEffect, useState } from "react";
import { observer } from "mobx-react-lite";
import { PlusOutlined } from "@ant-design/icons";
import { Tag } from "antd";

import {
  ChakraButtonProps,
  ColumnsType,
  FormItemProps,
  Panel,
  Operate,
} from "@hsu-react/ui";

import { UserData } from "@/services/apis/permit/user";
import UserStore from "./UserStore";
import UserForm from "./UserForm";
import ResetPasswordForm from "./ResetPasswordForm";
import styles from "./index.module.scss";

const User: React.FC = observer(() => {
  const {
    setSearchData,
    initSearchData,
    dataSource,
    isLoading,
    delData,
    getDataSource,
    total,
    changePage,
    page,
    resetUserPwd,
    order,
    onOrderChange,
  } = UserStore;
  const [open, setOpen] = useState<boolean>(false);
  const [title, setTitle] = useState<string>("新增");
  const [editUser, setEditUser] = useState<UserData | undefined>();
  const [resetPwd, setResetPwd] = useState<boolean>(false);
  const [pwdUsername, setPwdUsername] = useState<string>("");

  useEffect(() => {
    initSearchData();
  }, [initSearchData]);

  const searchItems: FormItemProps[] = [
    { type: "INPUT", name: "username", label: "用户名" },
  ];

  const beforeButtonGroup: ChakraButtonProps[] = [
    {
      title: "新增",
      colorPalette: "blue",
      icon: <PlusOutlined />,
      onClick: () => {
        setTitle("新增");
        setEditUser(undefined);
        setOpen(true);
      },
      hasPermi: ["permit:user:add"],
    },
  ];

  const columns: ColumnsType = [
    { title: "用户名", dataIndex: "username", width: 200 },
    { title: "昵称", dataIndex: "nickname", width: 200 },
    {
      title: "角色",
      dataIndex: "roleDsr",
      width: 120,
      align: "center",
      fixedWidth: true,
      render: (value: string, record: UserData) => (
        <Tag color={record.isSuper ? "purple" : "default"}>{value}</Tag>
      ),
    },
    {
      title: "设备数",
      dataIndex: "deviceCount",
      width: 100,
      align: "center",
      fixedWidth: true,
    },
    {
      title: "操作",
      width: 220,
      ellipsis: false,
      align: "center",
      fixed: "right",
      fixedWidth: true,
      render: (record: UserData) => (
        <Operate
          menu={[
            {
              title: "修改",
              onClick: () => {
                setTitle("修改");
                setEditUser(record);
                setOpen(true);
              },
              hasPermi: ["permit:user:upd"],
            },
            {
              title: "重置密码",
              onClick: () => {
                setPwdUsername(record.username ?? "");
                setResetPwd(true);
              },
              hasPermi: ["permit:user:resetPwd"],
            },
            {
              title: "删除",
              delete: true,
              // 超级管理员不可删除
              hidden: !!record.isSuper,
              onConfirm: () => {
                delData(record.username ?? "");
              },
              hasPermi: ["permit:user:del"],
            },
          ]}
        />
      ),
    },
  ];

  return (
    <>
      <Panel.List
        className={styles.User}
        searchProps={{
          searchItems,
          onSearch: setSearchData,
          onReset: initSearchData,
          beforeButtonGroup,
          hasPermi: ["permit:user:list"],
        }}
        tableProps={{
          columns,
          dataSource,
          rowKey: "id",
          loading: isLoading,
          pagination: {
            total,
            onChange: (num, size) => changePage({ num, size }),
            current: page?.num,
            pageSize: page?.size,
          },
          order,
          onOrderChange,
        }}
      />
      <UserForm
        open={open}
        title={title}
        data={editUser}
        onCancel={() => {
          setEditUser(undefined);
          setOpen(false);
        }}
        onOk={() => {
          getDataSource();
        }}
      />
      <ResetPasswordForm
        open={resetPwd}
        username={pwdUsername}
        onCancel={() => {
          setPwdUsername("");
          setResetPwd(false);
        }}
        onOk={(username, password, callback) => {
          resetUserPwd(username, password, callback);
        }}
      />
    </>
  );
});

export default User;
