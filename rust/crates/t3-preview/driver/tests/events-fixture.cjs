const fixture=require('./worker-fixture.cjs');
module.exports={chromium:{...fixture.chromium,launch:async(...args)=>{
  const browser=await fixture.chromium.launch(...args),create=browser.newContext;
  browser.newContext=async(...args)=>{
    process.stdout.write('malformed event JSON\n');
    process.stdout.write(JSON.stringify({event:'futureEvent',detail:'new protocol field'})+'\n');
    process.stdout.write(JSON.stringify({event:'status',input:{unexpected:true}})+'\n');
    return create(...args);
  };
  return browser;
}}};
