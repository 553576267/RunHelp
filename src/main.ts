import { createApp } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import App from './App.vue';
import AppMobile from './components/AppMobile.vue';
import './style.css';

// 移动端（Android/iOS）只挂载数据同步页；桌面端保持完整工作台。
// 后端 app_is_mobile 用编译期 cfg 判定，桌面构建该 command 同样存在，失败时回退桌面入口。
async function bootstrap() {
  let mobile = false;
  try {
    mobile = await invoke<boolean>('app_is_mobile');
  } catch {
    mobile = false;
  }
  createApp(mobile ? AppMobile : App).mount('#app');
}

void bootstrap();
