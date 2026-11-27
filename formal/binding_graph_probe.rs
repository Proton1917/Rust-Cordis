// Supporting stable snapshot APIs for the unmodified Manager::select_instances body.
#![allow(dead_code)]
use std::{any::Any, collections::{HashMap, HashSet}, sync::Arc};
mod cordis { pub type Realm = u32; }
#[derive(Clone)] struct Context(Arc<View>);
struct View { realm: u32, namespace: u32, parent: Option<Context>,
    configuration: Option<Arc<dyn Any + Send + Sync>>,
    bindings: HashMap<String, Arc<dyn Any + Send + Sync>> }
impl Context {
    fn fiber(&self) -> ParentFiber<'_> { ParentFiber(self) }
    fn get_named(&self, key: String) -> Result<Arc<dyn Any + Send + Sync>, ()> {
        self.lookup_named(key).ok_or(())
    }
    fn lookup_named(&self, key: String) -> Option<Arc<dyn Any + Send + Sync>> {
        self.0.bindings.get(&key).cloned()
    }
    fn realm_named(&self, _: &str) -> u32 { self.0.realm }
}
struct ParentFiber<'a>(&'a Context);
impl ParentFiber<'_> {
    fn config<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.0.0.configuration.clone()?.downcast().ok()
    }
    fn parent_context(&self) -> Option<Context> { self.0.0.parent.clone() }
}
struct Fiber { view: Context, parent: Option<Context> }
impl Fiber {
    fn parent_context(&self) -> Option<Context> { self.parent.clone() }
    fn context(&self) -> Context { self.view.clone() }
}
mod views { use super::*; pub fn realm_namespace(view: &Context) -> u32 {view.0.namespace} }
struct Lock<T>(T);
impl<T> Lock<T> { fn lock(&self) -> &T { &self.0 } }
struct Current(Option<Arc<Generation>>);
impl Current { fn upgrade(&self) -> Option<Arc<Generation>> {self.0.clone()} }
struct Instance { name: String, fiber: Fiber, data: InstanceData }
struct InstanceData { name: String, current: Lock<Current>, group: String,
    plugin: String, component: String }
struct Configuration { instance: InstanceName }
struct InstanceName { name: String }
struct Generation { component: Arc<RemoteComponent>, context: Context, instance: Arc<Instance> }
struct Binding { interface: Interface, generation: Option<Arc<Generation>> }
#[derive(Clone)] struct Interface {key: String, minor:u32}
impl Interface {
    fn parse(input: &str) -> Result<Self, ()> {
        let (key, minor) = input.rsplit_once('.').ok_or(())?;
        Ok(Self {key: key.to_owned(), minor: minor.parse().map_err(|_| ())?})
    }
}
struct Declaration { provides: Vec<String> }
struct Plugin { components: Vec<Declaration> }
struct RemoteComponent { inject: Vec<(Interface,bool)>, plugin: Plugin }
impl RemoteComponent { fn plugin_description(&self)->&Plugin {&self.plugin} }
struct Group { current: Lock<GroupData> }
struct GroupData { components:HashMap<(String,String),Arc<RemoteComponent>> }
struct Catalog { instances:HashMap<String,Arc<Instance>>, groups:HashMap<String,Arc<Group>> }
struct ManagerData { catalog:Lock<Catalog>, named:HashMap<u32,u32> }
struct Manager(ManagerData);
impl Manager {
    fn named_realms(&self)->HashMap<u32,u32>{ self.0.named.clone() }
}
mod production {
    use super::*;
    impl Manager {
        // @SOURCE:SELECT_INSTANCES@
    }
}

fn bit(mask:u32,index:u32)->bool {mask & (1<<index)!=0}
fn context(realm:u32, bindings:HashMap<String,Arc<dyn Any+Send+Sync>>)->Context {
    Context(Arc::new(View { realm, namespace:9, parent:None, configuration:None, bindings }))
}
fn component(key:&str, minor:u32, inject:bool, sibling:bool)->Arc<RemoteComponent> {
    let provides=if key.is_empty(){vec![]}else{vec![format!("{key}.{minor}")]};
    let mut declarations=vec![Declaration{provides:if sibling{vec![]}else{provides.clone()}}];
    if sibling{declarations.push(Declaration{provides});}
    Arc::new(RemoteComponent{inject:if inject{vec![(Interface{key:"service@1".into(),minor:2},true)]}else{vec![]},
        plugin:Plugin{components:declarations}})
}
fn instance(name:u32, view:Context, parent:Option<Context>, current:Option<Arc<Generation>>)->Arc<Instance>{
    Arc::new(Instance{name:name.to_string(),fiber:Fiber{view,parent},data:InstanceData{name:name.to_string(),current:Lock(Current(current)),
        group:name.to_string(),plugin:"plugin".into(),component:"component".into()}})
}
fn case(mask:u32)->String {
    let incoming=bit(mask,1);
    let actual_key=if bit(mask,2){"other@1"}else{"service@1"};
    let actual_minor=if bit(mask,3){1}else{3};
    let inherited=if bit(mask,7){1}else{2};
    let replacement=component(actual_key,actual_minor,false,false);
    let existing=if incoming{component("service@1",3,false,false)}else{replacement.clone()};
    let sibling=component(if bit(mask,14){"service@1"}else{""},3,false,true);
    let consumer=component("",3,true,false);
    let view0=context(inherited,HashMap::new());
    let view2=context(1,HashMap::new());
    let instance0=instance(0,view0.clone(),None,None);
    let instance2=instance(2,view2.clone(),None,None);
    let kind=mask>>10&3;
    let mut bindings:HashMap<String,Arc<dyn Any+Send+Sync>>=HashMap::new();
    if kind!=0 {
        let value:Arc<dyn Any+Send+Sync>=if bit(mask,9) {Arc::new("wrong dynamic type".to_owned())} else {
            Arc::new(Binding{interface:Interface{key:"service@1".into(),minor:if bit(mask,8){1}else{3}},
                generation:if kind==1 {None} else {Some(Arc::new(Generation{
                    component:existing.clone(),context:view0.clone(),
                    instance:if kind==2 {instance0.clone()}else{instance2.clone()}}))}})
        };
        bindings.insert("service@1".into(),value);
    }
    let view1=context(1,bindings);
    let parent=if bit(mask,0){
        let configured=Context(Arc::new(View{realm:1,namespace:9,parent:None,bindings:HashMap::new(),
            configuration:Some(Arc::new(Configuration{instance:InstanceName{name:"2".into()}}))}));
        Some(Context(Arc::new(View{realm:1,namespace:9,parent:Some(configured),configuration:None,bindings:HashMap::new()})))
    }else{None};
    let current=if bit(mask,4){Some(Arc::new(Generation{component:consumer.clone(),context:view1.clone(),instance:instance0.clone()}))}else{None};
    let instance1=instance(1,view1,parent,current);
    let mut groups=HashMap::new();
    for (name,value) in [(0,Some(existing)),(1,if bit(mask,5){None}else{Some(consumer)}),(2,Some(sibling))]{
        let mut components=HashMap::new();
        if let Some(value)=value{components.insert(("plugin".into(),"component".into()),value);}
        groups.insert(name.to_string(),Arc::new(Group{current:Lock(GroupData{components})}));
    }
    let instances=HashMap::from([("0".into(),instance0),("1".into(),instance1),("2".into(),instance2)]);
    let named=HashMap::from([(1,if bit(mask,6){9}else{8})]);
    let manager=Manager(ManagerData{catalog:Lock(Catalog{instances,groups}),named});
    let roots=[(12,"0"),(13,"2")].into_iter().filter(|(index,_)|bit(mask,*index)).map(|(_,name)|name.to_owned());
    let incoming=if incoming{vec![(view0,replacement,"0".into())]}else{vec![]};
    manager.select_instances(roots,incoming).iter().map(|value|value.data.name.clone()).collect::<Vec<_>>().join(",")
}
fn main(){for mask in 0..32768 {println!("{mask}\t{}",case(mask));}}
