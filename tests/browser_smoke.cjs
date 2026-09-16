// Browser integration regression test. No npm dependency in the app itself.
// PLAYWRIGHT_MODULE=/path/to/playwright-core node tests/browser_smoke.cjs [image.png]
// Runs its own server on 3333 and always stops it. Never touches the live unit.
const {chromium}=require(process.env.PLAYWRIGHT_MODULE || 'playwright-core');
const {spawn}=require('node:child_process');
const {once}=require('node:events');
const fs=require('node:fs/promises');
const os=require('node:os');
const path=require('node:path');
const assert=require('node:assert/strict');
const dir=path.resolve(__dirname,'..');
const base=process.env.MOSAIC_TEST_URL || 'http://127.0.0.1:3333/';
const ownServer=!process.env.MOSAIC_TEST_URL;
const server=ownServer?spawn(path.join(dir,'target/release/lego-mosaic'),[],{cwd:dir,env:{...process.env,LEGO_MOSAIC_ADDR:'127.0.0.1:3333'},stdio:['ignore','ignore','pipe']}):null;
let serverLog=''; server?.stderr.on('data',c=>serverLog+=c);
let browser;
(async()=>{
 for(let i=0;;i++){try{assert.equal(await (await fetch(new URL('healthz',base))).text(),'ok\n');break;}catch(e){if(i>100||server?.exitCode!=null)throw Error('Server not healthy: '+serverLog);await new Promise(r=>setTimeout(r,50));}}
 console.log('PASS health',base);
 browser=await chromium.launch({executablePath:process.env.CHROMIUM || '/usr/bin/chromium',headless:true,args:['--no-sandbox']});
 const page=await browser.newPage({acceptDownloads:true});
 const errors=[],requests=[]; page.on('pageerror',e=>errors.push(String(e))); page.on('request',r=>requests.push([r.method(),r.url()]));
 await page.goto(base);
 const ready=()=>page.waitForFunction(()=>!document.querySelector('#settings').disabled);
 await ready();
 const swatches=await page.locator('#exclusions').innerText();
 assert.match(swatches,/Yellow/);assert.doesNotMatch(swatches,/Brown/);
 await page.evaluate(async()=>{
  const c=document.createElement('canvas'); c.width=64;c.height=64;const x=c.getContext('2d');x.fillStyle='white';x.fillRect(0,0,64,64);x.fillStyle='black';x.fillRect(0,0,64,32);
  const blob=await new Promise(r=>c.toBlob(r));const d=new DataTransfer();d.items.add(new File([blob],'test.png',{type:'image/png'}));document.querySelector('#image').files=d.files;document.querySelector('#image').dispatchEvent(new Event('change',{bubbles:true}));
 }); await ready();
 async function set(id,val){await page.locator('#'+id).evaluate((el,v)=>{el.value=String(v);el.dispatchEvent(new Event('input',{bubbles:true}));el.dispatchEvent(new Event('change',{bubbles:true}));},val);}
 async function build(){await page.click('#go');await ready();assert.equal(await page.locator('#error').isVisible(),false,await page.locator('#error').innerText());await page.waitForFunction(()=>{const i=document.querySelector('#mosaic-image');return !i.hidden&&i.complete&&i.naturalWidth>0;});}
 async function svg(){return page.locator('#mosaic-image').evaluate(async i=>await(await fetch(i.src)).text());}
 await set('width',8);await set('height',8);await build();
 assert.equal(await page.locator('#rows td').count(),64);
 assert.equal(await page.locator('#rows tbody tr').first().locator('td span').first().innerText(),'01');
 assert.equal(await page.locator('#rows tbody tr').last().locator('td span').first().innerText(),'06');
 console.log('PASS known image, correct 32/32 count, bottom-up guide');
 const artifactDir=await fs.mkdtemp(path.join(os.tmpdir(),'mosaic-artifacts-'));
 async function save(id,name){const pending=page.waitForEvent('download');await page.click('#'+id);const d=await pending;assert.equal(d.suggestedFilename(),name);const file=path.join(artifactDir,name);await d.saveAs(file);assert.equal(await d.failure(),null);return await fs.readFile(file,'utf8');}
 let csv=await save('export-csv','mosaic-parts.csv');assert.match(csv,/"black","06".*"32"/);assert.match(csv,/"white","01".*"32"/);
 let guide=await save('export-guide','mosaic-guide.svg');assert.match(guide,/<svg/);assert.match(guide,/>06</);assert.match(guide,/Black/);
 await save('export-svg','mosaic.svg');
 await page.locator('#exclusions input[data-color="0"]').uncheck();
 assert.equal(await page.locator('#exclusions input[data-color="0"]').isChecked(),false);
 await set('palette','extended');await set('palette','mosaic-maker');assert.equal(await page.locator('#exclusions input[data-color="0"]').isChecked(),false);
 await build();csv=await save('export-csv','mosaic-parts.csv');assert.doesNotMatch(csv,/"white"/);assert.match(csv,/"black","06"/);
 for(const index of [1,2,3])await page.locator(`#exclusions input[data-color="${index}"]`).uncheck();
 await page.locator('#exclusions input[data-color="4"]').click();assert.equal(await page.locator('#exclusions input:checked').count(),1);assert.match(await page.locator('#error').innerText(),/at least one/);
 await page.click('#reset');
 console.log('PASS exports, stable symbols, persistent exclusions, all-excluded guard');
 if(process.argv[2]){
  await page.locator('#image').setInputFiles(path.resolve(process.argv[2]));await ready();await build();
  const flat=await svg();assert.match(flat,/<rect/);assert.doesNotMatch(flat,/<circle/);
  csv=await save('export-csv','mosaic-parts.csv');assert.match(csv,/"yellow","12"/);assert.match(csv,/"light-bluish-gray","03"/);
  console.log('PASS supplied image defaults:',await page.locator('#parts').innerText());
  await set('secondary_mode','preserve');await build();const natural=await svg();assert.notEqual(flat,natural);
  await page.click('#reset');await set('hue_mode','preserve');await build();assert.notEqual(flat,await svg());
  await page.click('#reset');await set('preview_style','stud');await build();assert.match(await svg(),/<circle/);
  await set('preset','photo');assert.equal(await page.locator('#secondary_mode').inputValue(),'preserve');assert.equal(await page.locator('#hue_mode').inputValue(),'preserve');await build();const dither=await svg();await page.evaluate(()=>document.querySelectorAll('details').forEach(d=>d.open=true));await page.locator('#dither').uncheck();await build();assert.notEqual(dither,await svg());
  await set('palette','extended');await set('width',128);await set('height',128);const t=Date.now();await build();console.log('PASS 128×128 extended photo build milliseconds',Date.now()-t);assert.equal(await page.locator('#rows td').count(),16384);
  await page.click('#reset');await set('preview_style','flat');await build();await save('export-svg','mosaic.svg');await page.screenshot({path:path.join(artifactDir,'studio-desktop.png'),fullPage:true});
  await page.setViewportSize({width:390,height:844});await page.screenshot({path:path.join(artifactDir,'studio-mobile.png'),fullPage:true});assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1),false);
  console.log('PASS family rules, photo dithering, styles, max dimensions and mobile layout');
 }
 // Failure recovery must preserve the previous result and allow another build.
 await page.locator('#image').setInputFiles({name:'broken.png',mimeType:'image/png',buffer:Buffer.from('not an image')});await ready();assert.equal(await page.locator('#error').isVisible(),true);await build();
 assert.equal(errors.length,0,errors.join('\n'));
 assert.equal(requests.some(([method])=>method!=='GET'),false,'No uploads or POST requests');
 assert.equal(requests.some(([,url])=>/^https?:/.test(url)&&!url.startsWith(new URL(base).origin)),false,'No third-party network calls');
 const failed=await browser.newPage();await failed.route('**/mosaic.js',r=>r.abort());await failed.goto(base);await failed.waitForFunction(()=>!document.querySelector('#error').hidden);assert.match(await failed.locator('#error').innerText(),/Could not load/);await failed.close();
 console.log('PASS decode failure recovery, missing-module error, local-only network');
 console.log('Artifacts:',artifactDir);
})().catch(e=>{console.error(e);process.exitCode=1}).finally(async()=>{await browser?.close();if(server&&server.exitCode===null){server.kill();await once(server,'exit');}});
