import {defineConfig,devices} from '@playwright/test';
export default defineConfig({
  testDir:'./tests',testMatch:'**/*.spec.js',fullyParallel:false,workers:1,timeout:45000,
  outputDir:'test-results',reporter:[['list'],['html',{open:'never'}]],
  use:{...devices['iPhone 13'],viewport:{width:390,height:844},deviceScaleFactor:1,browserName:'chromium',baseURL:'http://127.0.0.1:4173',trace:'retain-on-failure'},
  webServer:{command:'node dev/server.js',url:'http://127.0.0.1:4173',reuseExistingServer:false,timeout:10000},
});
