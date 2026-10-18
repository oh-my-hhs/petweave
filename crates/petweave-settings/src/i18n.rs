//! User-facing strings (F43: Chinese first, all in one place).
//!
//! A future language switch only needs to replace this module; the UI must
//! not hard-code Chinese labels elsewhere.

pub struct Tr;

impl Tr {
    pub const APP_TITLE: &'static str = "PetWeave 设置";
    pub const SIDEBAR_PETS: &'static str = "宠物列表";
    pub const SIDEBAR_PARAMS: &'static str = "参数";
    pub const SIDEBAR_ABOUT: &'static str = "关于";

    pub const SAVE: &'static str = "保存并应用";
    pub const RELOAD: &'static str = "重新加载";
    pub const SAVED: &'static str = "已保存并通知宿主重新加载";

    pub const PETS_HEADING: &'static str = "宠物";
    pub const ADD_PET: &'static str = "添加宠物";
    pub const ADD_BUILTIN: &'static str = "内置类型";
    pub const ADD_PACKAGE: &'static str = "已安装角色包";
    pub const NO_PACKAGES: &'static str = "（未安装角色包）";
    pub const COL_NAME: &'static str = "名字";
    pub const COL_KIND: &'static str = "类型";
    pub const COL_PACKAGE: &'static str = "角色包";
    pub const COL_STATE: &'static str = "状态";
    pub const COL_POSITION: &'static str = "位置";
    pub const COL_ACTIONS: &'static str = "操作";
    pub const ENABLED: &'static str = "已启用";
    pub const DISABLED: &'static str = "已停用";
    pub const ENABLE: &'static str = "启用";
    pub const DISABLE: &'static str = "停用";
    pub const MOVE_UP: &'static str = "上移";
    pub const MOVE_DOWN: &'static str = "下移";
    pub const COPY: &'static str = "复制";
    pub const DELETE: &'static str = "删除";

    pub const PARAMS_HEADING: &'static str = "参数";
    pub const NO_SELECTION: &'static str = "先在左侧列表选择一只宠物";
    pub const GROUP_GLOBAL: &'static str = "全局";
    pub const FPS: &'static str = "动画帧率";
    pub const GLOBAL_NOTE: &'static str = "全局设置对所有宠物生效。";
    pub const GROUP_GENERAL: &'static str = "通用";
    pub const GROUP_KIND: &'static str = "类型参数";
    pub const NAME: &'static str = "名字";
    pub const ENABLED_LABEL: &'static str = "启用";
    pub const KIND: &'static str = "类型";
    pub const PACKAGE: &'static str = "角色包";
    pub const OUTPUT: &'static str = "显示器";
    pub const OUTPUT_AUTO: &'static str = "自动";
    pub const LAYER: &'static str = "渲染层级";
    pub const MODE: &'static str = "移动模式";
    pub const INHERIT: &'static str = "继承";
    pub const INHERIT_RENDER: &'static str = "继承 [render]";
    pub const WIDTH: &'static str = "宽度";
    pub const HEIGHT: &'static str = "高度";
    pub const ANCHOR: &'static str = "锚点";
    pub const MARGINS: &'static str = "边距（上/右/下/左）";
    pub const CLICK_THROUGH: &'static str = "点击穿透";
    pub const FULLSCREEN_HIDE: &'static str = "全屏时自动隐藏";
    pub const BONGO_CAT_HEIGHT: &'static str = "猫高度";
    pub const BONGO_KEYPRESS_MS: &'static str = "按键保持（ms）";
    pub const BONGO_IDLE_SLEEP: &'static str = "闲置睡眠（秒，0=关）";
    pub const BONGO_SCHEDULED_SLEEP: &'static str = "定时睡眠";
    pub const BONGO_SLEEP_BEGIN: &'static str = "睡眠开始 HH:MM";
    pub const BONGO_SLEEP_END: &'static str = "睡眠结束 HH:MM";
    pub const BONGO_HAND_MAPPING: &'static str = "按物理左右手映射";
    pub const BONGO_MIRROR: &'static str = "水平镜像";
    pub const BONGO_ASSETS: &'static str = "素材目录";
    pub const DEMO_COLOR: &'static str = "颜色（#rrggbb）";
    pub const PACKAGE_ONLY: &'static str = "该类型的参数由角色包脚本/清单提供。";
    pub const DEFAULT: &'static str = "默认";

    pub const HOST_STATUS: &'static str = "宿主";
    pub const CONFIG_PATH: &'static str = "配置文件";
    pub const SOCKET_PATH: &'static str = "控制 socket";
    pub const PROTOCOL: &'static str = "控制协议";
    pub const RENDERER: &'static str = "渲染后端";
    pub const VERSION: &'static str = "版本";
    pub const PACKAGE_REPO: &'static str = "角色包仓库";
    pub const ABOUT_NOTE: &'static str =
        "设置面板是独立进程：关闭它不会影响桌面上的宠物；保存后宿主会热重载。";

    pub const UNSAVED: &'static str = "有未保存的修改";
    pub const CLOSE_CONFIRM: &'static str = "有未保存的修改，确定要关闭吗？";
    pub const SAVE_AND_CLOSE: &'static str = "保存并关闭";
    pub const DISCARD: &'static str = "放弃修改";
    pub const CANCEL: &'static str = "取消";
    pub const MIGRATED: &'static str = "配置已从旧 [pet] 格式升级为 [[pets]]，保存后写入文件。";
    pub const UNKNOWN_KEYS: &'static str = "未知配置键（已忽略）";
    pub const INVALID_CONFIG: &'static str = "配置校验失败";
    pub const HOST_OFFLINE: &'static str =
        "宠物未运行：仍可编辑并保存，下次启动生效。控制相关操作已禁用。";
}
