// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License
// Real-photo QA. Supply local photo paths; nothing is uploaded to the server.
const {chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright-core');
const {spawn,execFileSync}=require('node:child_process');
const fs=require('node:fs/promises'),path=require('node:path'),assert=require('node:assert/strict');
const root=path.resolve(__dirname,'..'),base='http://127.0.0.1:3334/';
const server=spawn(path.join(root,'target/release/lego-mosaic'),[],{env:{...process.env,LEGO_MOSAIC_ADDR:'127.0.0.1:3334'},stdio:'ignore'});
let browser;
(async()=>{
 for(let i=0;;i++){try{await fetch(base);break;}catch(e){if(i>60)throw e;await new Promise(r=>setTimeout(r,100));}}
 browser=await chromium.launch({executablePath:'/usr/bin/chromium',headless:true,args:['--no-sandbox']});
 const page=await browser.newPage({viewport:{width:1280,height:960}});
 await page.goto(base);const ready=()=>page.waitForFunction(()=>!document.querySelector('#settings').disabled,{},{timeout:120000});await ready();
 const set=(id,v)=>page.locator('#'+id).evaluate((e,v)=>{e.value=String(v);e.dispatchEvent(new Event('input',{bubbles:true}));e.dispatchEvent(new Event('change',{bubbles:true}));},v);
 const out=await fs.mkdtemp('/tmp/mosaic-photos-');
 for(const image of process.argv.slice(2)){
  await page.locator('#image').setInputFiles(path.resolve(image));await ready();await set('preset','photo');await set('palette','extended');await set('width',64);await set('height',64);
  await page.evaluate(()=>document.querySelectorAll('details').forEach(d=>d.open=true));
  for(const fit of ['contain','crop','stretch']){
   await set('fit',fit);await page.click('#go');await ready();assert.equal(await page.locator('#error').isVisible(),false);
   const svg=await page.locator('#mosaic-image').evaluate(async i=>await(await fetch(i.src)).text());
   const name=path.basename(image,'.png')+'-'+fit;const cli=path.join(out,name+'.svg');
   execFileSync(path.join(root,'target/release/lego-mosaic'),['convert',path.resolve(image),'--output',cli,'--preset','photo','--palette','extended','--size','64','--fit',fit]);
   assert.equal(svg,await fs.readFile(cli,'utf8'),'exact real-photo parity');
   await page.locator('.preview-grid').screenshot({path:path.join(out,name+'.png')});
   console.log('PASS photo',name,'CLI/worker equality and preview');
  }
 }
 console.log('Photo artifacts:',out);
})().catch(e=>{console.error(e);process.exitCode=1}).finally(async()=>{await browser?.close();server.kill();});
