/* VRG local RAG graph: native WebGL2, progressive authorized snapshots. MIT OR Apache-2.0. */
'use strict';
(() => {
  const $=id=>document.getElementById(id);
  const es = {
    skip:'Ir a lista accesible',title:'Explorador RAG',readonly:'LOCAL · SOLO LECTURA',refresh:'Actualizar',language:'Idioma',scope:'Ámbito autorizado',project:'Proyecto',choose:'Selecciona un proyecto',search:'Buscar texto, título o ruta',filters:'Filtros del lote cargado',root:'Raíz',document:'Documento',kind:'Tipo de nodo',all:'Todos',documents:'Documentos',summaries:'Resúmenes derivados',archives:'Revisiones archivadas',tier:'Clasificación',unclassified:'Sin clasificar',revision:'Revisión',current:'Actual',historical:'Histórica',connectivity:'Conectividad',isolated:'Aislados',connected:'Conectados',edges:'Aristas',contains:'Contención',explicit:'Enlace explícito',similarity:'Similitud calculada',derived:'Procedencia derivada',reset:'Restablecer filtros',view:'Vista',structure:'Estructural',embeddings:'Proyección de embeddings',structuralNote:'Disposición estructural; la distancia no mide similitud.',fit:'Ajustar',focus:'Enfocar vecinos',unfocus:'Deshacer enfoque',pause:'Detener layout',resume:'Reanudar layout',initial:'Los chunks de un proyecto autorizado aparecerán aquí.',previous:'Lote anterior',next:'Lote siguiente',list:'Lista accesible de nodos',listHint:'Selecciona un nodo para consultar su texto, revisión y relaciones. Los filtros se aplican al lote cargado.',previousList:'Anterior',nextList:'Siguiente',inspector:'Inspector',inspectHint:'Selecciona un chunk del grafo o de la lista.',readChunk:'Leer chunk',readSource:'Leer fuente exacta',copy:'Copiar referencia',moreText:'Leer página siguiente',relations:'Relaciones en el lote',footer:'Datos locales · Sin edición, indexación ni inferencia al visualizar',loading:'Leyendo lote autorizado…',empty:'EMPTY_INDEX · El índice no contiene nodos autorizados. La indexación es una operación separada de VRG.',filterEmpty:'FILTER_EMPTY · No hay nodos con estos filtros en el lote. Restablece los filtros o cambia de lote.',noProjects:'0 proyectos registrados. Selecciona un perfil ya autorizado mediante --config.',noVectors:'NO_VECTORS · Vista estructural disponible; no hay embeddings compatibles en este lote.',sessionMissing:'Sesión ausente o caducada. Abre de nuevo el enlace de sesión que imprime vrg; no lo compartas.',newGeneration:'Hay una generación nueva disponible. Pulsa Actualizar para cambiar el snapshot completo.',revoked:'Permiso o perfil modificado. Se ha limpiado la vista. Actualiza para resolver el ámbito vigente.',copied:'Referencia copiada.',copyFailed:'No se pudo copiar. Puedes seleccionar la revisión y ruta en el inspector.',noRelations:'Sin relaciones en el lote cargado.',loaded:'cargados',authorized:'autorizados',visible:'visibles',partial:'PARCIAL',complete:'completo',selection:'selección',nodeType:'Tipo',source:'Fuente',offsets:'Offsets',offsetBasis:'Base de offsets',tierOrigin:'Origen del tier',content:'Representación',id:'ID de nodo',chunk:'Chunk',pin:'Pin',provenance:'Procedencia',model:'Modelo',bytes:'Bytes de texto',page:'Página',exact_chunk:'Texto exacto del chunk',source_metadata:'Metadatos de fuente',derived_summary:'Resumen derivado · no es el original',archive_revision:'Revisión archivada exacta',rawSource:'Markdown original exacto · revisión verificada',none:'No consta',yes:'Sí',no:'No',limit:'Límite de vista alcanzado; cambia de lote o acota la búsqueda.',clear:'Cerrar inspector',sourceMismatch:'SOURCE_CHANGED · La fuente actual no coincide con esta revisión.',readError:'No se pudo leer. Actualiza o comprueba el perfil mediante la CLI.',statusUnknown:'No se pudo comprobar el estado actual; la vista se ha limpiado para evitar servir datos con permisos obsoletos.',projectLabel:'Proyecto',generationLabel:'Generación',policyLabel:'Política',scopeDocument:'documento',scopeChunk:'chunk',scopeSummary:'resumen',scopeComputed:'calculado',score:'coseno',focusHint:'Enfoque local: selección y vecinos cargados',loadingDetail:'Leyendo texto autorizado…',noText:'Este nodo no contiene texto en esta representación.',unavailable:'No disponible',projectionSample:'Proyección del subconjunto cargado; la distancia 2D es aproximada.',sourceHint:'La base de offsets del chunk es body_text_utf8; no se aplican sus rangos al Markdown original.'
  };
  const en = {
    skip:'Skip to accessible list',title:'RAG explorer',readonly:'LOCAL · READ ONLY',refresh:'Refresh',language:'Language',scope:'Authorized scope',project:'Project',choose:'Choose a project',search:'Search text, title or path',filters:'Filters on the loaded batch',root:'Root',document:'Document',kind:'Node type',all:'All',documents:'Documents',summaries:'Derived summaries',archives:'Archived revisions',tier:'Classification',unclassified:'Unclassified',revision:'Revision',current:'Current',historical:'Historical',connectivity:'Connectivity',isolated:'Isolated',connected:'Connected',edges:'Edges',contains:'Containment',explicit:'Explicit link',similarity:'Computed similarity',derived:'Derived provenance',reset:'Reset filters',view:'View',structure:'Structural',embeddings:'Embedding projection',structuralNote:'Structural layout; distance does not measure similarity.',fit:'Fit',focus:'Focus neighbors',unfocus:'Undo focus',pause:'Stop layout',resume:'Resume layout',initial:'Chunks from an authorized project will appear here.',previous:'Previous batch',next:'Next batch',list:'Accessible node list',listHint:'Select a node to inspect its text, revision and relations. Filters apply to the loaded batch.',previousList:'Previous',nextList:'Next',inspector:'Inspector',inspectHint:'Select a chunk in the graph or list.',readChunk:'Read chunk',readSource:'Read exact source',copy:'Copy reference',moreText:'Read next page',relations:'Relations in the batch',footer:'Local data · No editing, indexing or inference while viewing',loading:'Reading authorized batch…',empty:'EMPTY_INDEX · The index has no authorized nodes. Indexing is a separate VRG operation.',filterEmpty:'FILTER_EMPTY · No nodes match these filters in this batch. Reset filters or change batch.',noProjects:'0 registered projects. Select an already authorized profile with --config.',noVectors:'NO_VECTORS · Structural view available; no compatible embeddings in this batch.',sessionMissing:'Missing or expired session. Reopen the session link printed by vrg; do not share it.',newGeneration:'A newer generation is available. Refresh to switch the complete snapshot.',revoked:'Permission or profile changed. The view was cleared. Refresh to resolve the current scope.',copied:'Reference copied.',copyFailed:'Could not copy. You can select the revision and path in the inspector.',noRelations:'No relations in the loaded batch.',loaded:'loaded',authorized:'authorized',visible:'visible',partial:'PARTIAL',complete:'complete',selection:'selection',nodeType:'Type',source:'Source',offsets:'Offsets',offsetBasis:'Offset basis',tierOrigin:'Tier origin',content:'Representation',id:'Node ID',chunk:'Chunk',pin:'Pin',provenance:'Provenance',model:'Model',bytes:'Text bytes',page:'Page',exact_chunk:'Exact chunk text',source_metadata:'Source metadata',derived_summary:'Derived summary · not the original',archive_revision:'Exact archived revision',rawSource:'Exact original Markdown · verified revision',none:'Not recorded',yes:'Yes',no:'No',limit:'View limit reached; change batch or narrow the search.',clear:'Close inspector',sourceMismatch:'SOURCE_CHANGED · Current source does not match this revision.',readError:'Could not read. Refresh or check the profile using the CLI.',statusUnknown:'Current state could not be checked; the view was cleared to avoid showing data with stale permissions.',projectLabel:'Project',generationLabel:'Generation',policyLabel:'Policy',scopeDocument:'document',scopeChunk:'chunk',scopeSummary:'summary',scopeComputed:'computed',score:'cosine',focusHint:'Local focus: selection and loaded neighbors',loadingDetail:'Reading authorized text…',noText:'This node contains no text in this representation.',unavailable:'Unavailable',projectionSample:'Projection of the loaded subset; 2D distances are approximate.',sourceHint:'Chunk offsets use body_text_utf8; they do not address the original Markdown.'
  };
  es.exact_source = 'Fuente exacta'; en.exact_source = 'Exact source';
  es.filterEmpty = 'FILTER_EMPTY · No hay nodos con estos filtros en el lote. Cambia el tipo de nodo, los filtros o el lote para consultar chunks, resúmenes y revisiones archivadas.';
  en.filterEmpty = 'FILTER_EMPTY · No nodes match these filters in this batch. Change node type, filters or batch to inspect chunks, summaries and archived revisions.';
  Object.assign(es,{projects:'proyectos',allProjects:'Todos los proyectos',expandGraph:'Ampliar grafo',collapseGraph:'Reducir grafo · Esc',collectionNote:'Vista conjunta de proyectos autorizados con snapshots independientes. Sin relaciones ni similitud entre proyectos. Relleno: clasificación; borde: proyecto.',tierCounts:'Clasificación visible'});
  Object.assign(en,{projects:'projects',allProjects:'All projects',expandGraph:'Expand graph',collapseGraph:'Restore graph · Esc',collectionNote:'Authorized projects with independent snapshots. No relationships or similarity between projects. Fill: classification; outline: project.',tierCounts:'Visible classification'});
  Object.assign(es,{filters:'Filtros de todos los chunks cargados',listHint:'Lista paginada accesible; todos los puntos permanecen en la GPU.',collectionNote:'Todos los chunks autorizados, cargados progresivamente. Relaciones conservan su proyecto y procedencia.',structuralNote:'Grupos estables por proyecto y carpeta; líneas semánticas calculadas sobre embeddings existentes. La distancia visual no es un score.',allScope:'Volver a todos',tree:'Proyectos y carpetas',color:'Color por',folder:'Carpeta',index:'Chunks en índice',rendered:'Representados GPU',filtered:'Filtrados',failed:'Proyectos no disponibles',loading:'Cargando todos los chunks autorizados…',complete:'Carga completa',list:'Lista accesible de chunks',relations:'Relaciones verificables',webgl:'WebGL2 no está disponible. Activa la aceleración gráfica del navegador; los datos siguen disponibles en la lista.',pause:'Posiciones estables',noRelations:'Sin relaciones registradas para este chunk.'});
  Object.assign(en,{filters:'Filters on all loaded chunks',listHint:'Paged accessible list; all points remain on the GPU.',collectionNote:'All authorized chunks, progressively loaded. Edges retain their project and provenance.',structuralNote:'Stable project and folder groups; semantic edges calculated from existing embeddings. Visual distance is not a score.',allScope:'Return to all',tree:'Projects and folders',color:'Color by',folder:'Folder',index:'Indexed chunks',rendered:'GPU represented',filtered:'Filtered',failed:'Unavailable projects',loading:'Loading all authorized chunks…',complete:'Load complete',list:'Accessible chunk list',relations:'Verifiable relations',webgl:'WebGL2 is unavailable. Enable browser graphics acceleration; data remains available in the list.',pause:'Stable positions',noRelations:'No recorded relations for this chunk.'});
  Object.assign(es,{nucleiLayout:'Disposición de núcleos',nucleusSeparation:'Separación',nucleusDensity:'Densidad interna',backNucleus:'Volver',toggleInspector:'Inspector',minimap:'Minimapa',compact:'Compacta',wide:'Amplia',loose:'Aireada',dense:'Densa'});
  Object.assign(en,{nucleiLayout:'Nucleus layout',nucleusSeparation:'Separation',nucleusDensity:'Internal density',backNucleus:'Back',toggleInspector:'Inspector',minimap:'Minimap',compact:'Compact',wide:'Wide',loose:'Airy',dense:'Dense'});
  const ALL_PROJECTS='__all__', PAGE_SIZE=10000, LIST_SIZE=50, TREE_PAGE=60;
  let lang='es',expanded=false;
  const t=key=>(lang==='en'?en:es)[key]||key;
  const text=(id,value)=>{if($(id))$(id).textContent=value;};
  const fragment=new URLSearchParams(location.hash.slice(1));
  let token=fragment.get('token')||(location.hash.length>1&&!location.hash.includes('=')?location.hash.slice(1):'');
  history.replaceState(null,'',location.pathname);
  const state={project:'',projects:[],mapping:null,data:null,nodes:[],edges:[],visible:[],visibleEdges:[],nodeMap:new Map(),edgeKeys:new Map(),positions:new Map(),selected:null,detail:null,sourceMode:false,focused:null,listPage:0,request:0,detailRequest:0,controller:null,detailController:null,scale:1,panX:0,panY:0,fitScale:1,polling:false,loading:false,scopes:new Set(),tree:null,treeMap:new Map(),treeExpanded:new Set(['all']),treePages:new Map(),projectStats:new Map(),loadedMemberships:0,failures:[],lastPaint:0,dirty:true,rendered:0,renderedEdges:0,frame:0,paused:true};
  const canvas=$('graph');
  const sourceOf=node=>node.source||{};
  const canonicalOf=p=>p.canonical_project_id||p.id||p.project_id;
  const canonicalLabel=p=>p.canonical_project_label||p.label||p.name||canonicalOf(p);
  const visualProjects=()=>[...new Map(state.projects.map(p=>[canonicalOf(p),{id:canonicalOf(p),label:canonicalLabel(p)}])).values()].sort((a,b)=>a.id.localeCompare(b.id));
  const nucleusOf=n=>n.nucleus_id||n.project_id;
  const projectName=id=>visualProjects().find(p=>p.id===id)?.label||canonicalLabel(state.projects.find(p=>(p.id||p.project_id)===id)||{id});
  state.originHierarchies=new Map();state.navigation={projectId:null,folderKey:null};state.nuclei=null;state.overlayLabels=[];state.overviewScale=null;state.cameraAnimation=0;
  const kindName=kind=>({chunk:'chunk',document:t('document'),summary:t('summaries'),source_revision:t('archives')})[kind]||kind;
  const short=value=>String(value||'').slice(0,12);
  const scalar=value=>value==null?t('none'):typeof value==='object'?JSON.stringify(value):String(value);
  const projectEndpoint=(id,tail)=>'/api/v1/projects/'+encodeURIComponent(id)+'/'+tail;
  const projected=node=>node.position&&Number.isFinite(Number(node.position.x))&&Number.isFinite(Number(node.position.y));
  const isPca=()=>false;
  const graphNodes=()=>state.visible;
  function message(id,value){text(id,value||'');$(id).hidden=!value;}
  function errorCode(error){return error?.code||error?.message||'INTERNAL_ERROR';}
  async function api(path,signal){
    if(!token)throw Object.assign(new Error('UNAUTHORIZED'),{code:'UNAUTHORIZED'});
    const response=await fetch(path,{headers:{Authorization:'Bearer '+token,Accept:'application/json'},cache:'no-store',credentials:'omit',redirect:'error',signal});
    let body;try{body=await response.json();}catch(_){throw Object.assign(new Error('INVALID_RESPONSE'),{code:'INVALID_RESPONSE'});}
    if(!response.ok)throw Object.assign(new Error(body?.error?.code||body?.code||'INTERNAL_ERROR'),{code:body?.error?.code||body?.code||'INTERNAL_ERROR'});
    return body;
  }
  function clearInspector(){
    state.detailRequest++;state.detailController?.abort();state.selected=null;state.detail=null;
    $('inspect-content').hidden=true;$('inspect-empty').hidden=false;text('node-text','');text('node-title','');text('text-kind','');$('metadata').replaceChildren();$('relations').replaceChildren();$('focus').disabled=true;draw();
  }
  function clearGraph(){
    state.request++;state.controller?.abort();clearInspector();state.nodes=[];state.edges=[];state.visible=[];state.visibleEdges=[];state.nodeMap.clear();state.edgeKeys.clear();state.originHierarchies?.clear();state.positions.clear();state.data=null;state.tree=null;state.treeRows=null;state.pickOrder=[];hideTooltip();state.treeMap.clear();state.scopes.clear();state.focused=null;state.projectStats.clear();state.loadedMemberships=0;state.failures=[];state.loading=false;state.rendered=0;state.dirty=true;state.layoutCount=0;state.treeMembershipCount=0;state.fitScale=1;state.overviewScale=null;state.nuclei=null;state.folderPositions=new Map();state.navigation={projectId:null,folderKey:null};
    $('node-list').replaceChildren();$('scope-tree').replaceChildren();$('project-legend').replaceChildren();$('folder-legend').replaceChildren();text('coverage','');text('graph-counts','');text('visible-count','');text('tier-counts','');text('generation','');text('sync-state','');$('unfocus').disabled=true;$('empty-state').hidden=false;text('empty-state',t('initial'));$('graph-view').setAttribute('aria-busy','false');draw();
  }
  function showError(error){
    if(error.name==='AbortError')return;const code=errorCode(error);
    if(['ROOT_REVOKED','SOURCE_CHANGED','SCOPE_MISMATCH','PROFILE_NOT_FOUND','SNAPSHOT_EXPIRED','POLICY_CHANGED'].includes(code))clearGraph();
    if(code==='UNAUTHORIZED'){token='';clearGraph();message('error',t('sessionMissing'));}
    else message('error',code+' · '+(code==='SOURCE_CHANGED'?t('sourceMismatch'):t('readError')));
  }
  function translate(){
    document.documentElement.lang=lang;for(const el of document.querySelectorAll('[data-i18n]'))el.textContent=t(el.dataset.i18n);
    const all=$('projects').querySelector('option[value="__all__"]');if(all)all.textContent=t('allProjects');
    text('expand-graph',t(expanded?'collapseGraph':'expandGraph'));renderMapping();if(state.data){renderHeader();renderTree();renderProjects();applyFilters(false);if(state.selected)renderInspector(state.detail);}
  }
  function renderMapping(){
    text('mapping',lang==='en'?'Already-authorized local project scopes':'Ámbitos locales de proyecto ya autorizados');
  }
  async function boot(){
    if(!token){message('error',t('sessionMissing'));return;}
    try{
      const session=await api('/api/v1/session'),result=await api('/api/v1/projects');state.projects=Array.isArray(result)?result:result.projects||[];window.VrgNuclei.register(visualProjects());
      $('projects').disabled=false;const all=document.createElement('option');all.value=ALL_PROJECTS;all.textContent=t('allProjects');$('projects').append(all);
      for(const p of visualProjects()){const o=document.createElement('option');o.value=p.id;o.textContent=p.label;$('projects').append(o);}
      state.mapping=result.collection_mapping||session.collection_mapping;renderMapping();
      if(!state.projects.length){message('notice',t('noProjects'));return;}
      $('projects').value=ALL_PROJECTS;await changeProject();
    }catch(error){showError(error);}
  }
  async function changeProject(){
    if(state.project===ALL_PROJECTS&&state.data){const chosen=$('projects').value;state.scopes.clear();if(chosen&&chosen!==ALL_PROJECTS)state.scopes.add(keyOf([chosen]));applyFilters();renderTree();const group=state.nuclei?.projects.find(p=>p.id===chosen);if(group)focusNucleus(group);else fitGraph();return;}
    clearGraph();state.project=$('projects').value;message('error','');message('notice','');$('project-controls').hidden=!state.project;$('refresh').disabled=!state.project;$('mode').disabled=true;$('search').value='';resetFilters(false);if(state.project)await loadGraph();
  }
  async function loadGraph(){
    if(!state.project)return;
    const project=state.project;clearGraph();state.project=project;state.controller=new AbortController();const request=++state.request,signal=state.controller.signal;
    state.loading=true;$('graph-view').setAttribute('aria-busy','true');message('error','');message('notice',t('loading'));
    state.data={snapshot_id:'collection-'+request,snapshots:{},projection:{method:'folder_semantic',note:t('structuralNote')},coverage:{partial:true},generation_id:'independent',policy_fingerprint:'independent'};
    const projects=project===ALL_PROJECTS?state.projects:state.projects.filter(p=>canonicalOf(p)===project);
    const progress=async(force=false)=>{
      if(request!==state.request)return;
      renderHeader();const nodeGrowth=state.nodes.length-(state.layoutCount||0),edgeGrowth=state.edges.length-(state.paintedEdgeCount||0),scopeDone=[...state.projectStats.values()].some(p=>p.done&&!p.painted);if(force||!state.layoutCount&&state.nodes.length||scopeDone||nodeGrowth>=Math.max(10000,state.nodes.length*.25)||edgeGrowth>=Math.max(50000,state.edges.length*.25)){for(const p of state.projectStats.values())if(p.done)p.painted=true;state.paintedEdgeCount=state.edges.length;state.lastPaint=performance.now();if(state.treeMembershipCount!==state.loadedMemberships){buildTree();state.treeMembershipCount=state.loadedMemberships;renderTree();renderProjects();}if(force||state.layoutCount!==state.nodes.length){layout();state.layoutCount=state.nodes.length;}applyFilters(false);renderHeader();if(state.nodes.length&&state.fitScale===1)fitGraph();await new Promise(resolve=>requestAnimationFrame(resolve));}
    };
    try{
      // Each project keeps its own authorized snapshot. Paging never discards an earlier page.
      for(const p of projects){
        const id=p.id||p.project_id,label=canonicalLabel(p),nucleusId=canonicalOf(p);let cursor=null,snapshot=null,seen=new Set(),batch=PAGE_SIZE;
        try{
          do{
            const q=new URLSearchParams({limit:String(batch),mode:'full'});if(cursor)q.set('cursor',cursor);if(snapshot)q.set('snapshot_id',snapshot);
            let data;for(;;){try{data=await api(projectEndpoint(id,'graph')+'?'+q,signal);break;}catch(error){if(errorCode(error)!=='LIMIT_EXCEEDED'||batch===1)throw error;batch=Math.max(1,Math.floor(batch/2));q.set('limit',String(batch));}}if(request!==state.request)return;
            for(const [identity,paths]of Object.entries(data.origin_hierarchies||{}))state.originHierarchies.set(id+':'+identity,paths);
            if(snapshot&&snapshot!==data.snapshot_id)throw Object.assign(new Error('SNAPSHOT_EXPIRED'),{code:'SNAPSHOT_EXPIRED'});snapshot=data.snapshot_id;
            state.data.snapshots[id]={snapshot_id:snapshot,generation_id:data.generation_id,policy_fingerprint:data.policy_fingerprint};
            if(project!==ALL_PROJECTS){state.data.snapshot_id=snapshot;state.data.generation_id=data.generation_id;state.data.policy_fingerprint=data.policy_fingerprint;}
            state.projectStats.set(id,{id,label,painted:state.projectStats.get(id)?.painted||false,...data.coverage,loaded:(state.projectStats.get(id)?.loaded||0)+(data.nodes||[]).filter(n=>n.kind==='chunk').length,done:!data.next_cursor,warning_codes:[...new Set([...(state.projectStats.get(id)?.warning_codes||[]),...(data.warnings||[]).map(w=>w.code).filter(Boolean)])]});
            for(const n of data.nodes||[]){
              if(n.kind!=='chunk')continue;const alias=id+':'+n.id;if(state.nodeMap.has(alias))continue;
              const key=n.canonical_source_identity&&n.native_chunk_id?'chunk:'+JSON.stringify([nucleusId,n.canonical_source_identity,n.native_chunk_id]):alias;
              const member={project_id:id,project_label:label,nucleus_id:nucleusId,nucleus_label:label,native_id:n.id,source:n.source,origin:n.origin,hierarchy_path:n.hierarchy_path};
              let node=state.nodeMap.get(key);if(!node){node={...n,native_id:n.id,id:key,project_id:id,project_label:label,nucleus_id:nucleusId,nucleus_label:label,memberships:[]};state.nodes.push(node);state.nodeMap.set(key,node);}const origins=state.originHierarchies.get(id+':'+n.canonical_source_identity);if(origins?.length){for(const origin of origins)node.memberships.push({...member,hierarchy_path:origin.hierarchy_path,origin:{...member.origin,original_root:origin.original_root,relative_path:origin.relative_path},originSelection:origin});}else node.memberships.push(member);state.nodeMap.set(alias,node);state.loadedMemberships++;
            }
            for(const e of data.edges||[]){
              const source=state.nodeMap.get(id+':'+e.source)?.id,target=state.nodeMap.get(id+':'+e.target)?.id;if(!source||!target||source===target)continue;
              const key=JSON.stringify([e.kind,source,target,e.score,e.model_identity,e.provenance_scope,e.original_link_kind]);
              if(state.edgeKeys.has(key)){const prior=state.edgeKeys.get(key);if(!prior.project_ids.includes(id))prior.project_ids.push(id);continue;}
              const edge={...e,id:key,source,target,project_id:id,nucleus_id:nucleusId,rank:hash(key),project_ids:[id]};state.edgeKeys.set(key,edge);state.edges.push(edge);
            }
            cursor=data.next_cursor||null;if(cursor){if(seen.has(cursor))throw Object.assign(new Error('INVALID_CURSOR'),{code:'INVALID_CURSOR'});seen.add(cursor);}
            await progress();
          }while(cursor);
        }catch(error){
          if(error.name==='AbortError'||['UNAUTHORIZED','ROOT_REVOKED','POLICY_CHANGED','SCOPE_MISMATCH','SNAPSHOT_EXPIRED','SOURCE_CHANGED'].includes(errorCode(error)))throw error;
          state.failures.push({id,label,code:errorCode(error)});
        }
      }
      // A collection is only complete after every included independent snapshot is revalidated.
      for(const [id,scope]of Object.entries(state.data.snapshots)){
        const status=await api(projectEndpoint(id,'status'),signal);if(status.changed||status.snapshot_id!==scope.snapshot_id||status.policy_fingerprint!==scope.policy_fingerprint)throw Object.assign(new Error('SNAPSHOT_EXPIRED'),{code:'SNAPSHOT_EXPIRED'});
      }
      if(request!==state.request)return;state.loading=false;state.data.coverage.partial=!!state.failures.length;await progress(true);fitGraph();
      message('notice',state.failures.length?t('failed')+': '+state.failures.map(p=>p.label+' ('+p.code+')').join(', '):t('complete'));
    }catch(error){if(request!==state.request||error.name==='AbortError')return;clearGraph();showError(error);}
    finally{if(request===state.request){state.loading=false;$('graph-view').setAttribute('aria-busy','false');renderHeader();}}
  }
  function renderHeader(){
    if(!state.data)return;text('project-title',state.project===ALL_PROJECTS?t('allProjects'):projectName(state.project));
    text('generation',visualProjects().length+' '+t('projects')+' · '+Object.keys(state.data.snapshots).length+' snapshots'+' · '+(lang==='en'?'authorized independent snapshots':'snapshots autorizados independientes'));
    text('projection-note',lang==='en'?'Project → folder → real chunk. Spatial distance does not measure semantic similarity.':'Proyecto → carpeta → chunk real. La distancia espacial no mide similitud semántica.');text('sync-state',state.loading?t('loading'):state.failures.length?t('partial'):t('complete'));
    let indexed=0,authorized=0,known=0,stale=0,denied=0,outside=0;const warningCodes=new Set();for(const p of state.projectStats.values()){indexed+=Number(p.chunks_total_indexed||0);authorized+=Number(p.chunks_total_authorized??p.nodes_total_authorized??0);stale+=Number(p.chunks_stale||0);denied+=Number(p.chunks_denied||0);outside+=Number(p.chunks_outside_authorized_roots||0);for(const code of p.warning_codes||[])warningCodes.add(code);known++;}
    const expected=state.project===ALL_PROJECTS?state.projects.length:1,unknown=known<expected||state.failures.length;
    const format=n=>n.toLocaleString(lang==='en'?'en-US':'es-MX');
    text('graph-counts',(lang==='en'?'Indexed entries':'Entradas de índice')+': '+format(indexed)+(unknown?' + ?':'')+' · '+(lang==='en'?'Authorized entries':'Entradas autorizadas')+': '+format(authorized)+(unknown?' + ?':'')+' · '+(lang==='en'?'Loaded index entries':'Entradas cargadas')+': '+format(state.loadedMemberships)+' · '+(lang==='en'?'Unique chunks loaded':'Chunks únicos cargados')+': '+format(state.nodes.length)+' · GPU: '+format(state.gpuResident||0)+' · '+t('rendered')+' LOD: '+format(state.rendered)+' · '+t('visible')+': '+format(state.inViewport||0)+' · '+t('filtered')+': '+format(state.nodes.length-state.visible.length)+((stale||denied||outside)?' · '+(lang==='en'?'Excluded: stale ':'Excluidos: desactualizados ')+format(stale)+' / '+(lang==='en'?'denied ':'denegados ')+format(denied)+' / '+(lang==='en'?'outside authorized roots ':'fuera de raíces autorizadas ')+format(outside):'')+(warningCodes.size?' · '+[...warningCodes].join(', '):''));
    text('coverage',(state.loading?t('loading'):state.failures.length?t('partial'):t('complete'))+' · '+state.edges.length.toLocaleString()+' '+t('edges')+' · '+(state.renderedEdges||0).toLocaleString()+' '+(lang==='en'?'lines drawn (zoom detail)':'líneas dibujadas (detalle por zoom)'));
    const bar=$('load-progress');bar.max=Math.max(authorized,state.loadedMemberships,1);bar.value=state.loadedMemberships;bar.setAttribute('aria-label',t('loaded')+' / '+t('authorized'));
    $('semantic-option').disabled=true;
  }
  function enabledEdges(){return new Set([...document.querySelectorAll('fieldset input:checked')].map(i=>i.value));}
  function fillFilters(){} // The lazy hierarchy replaces document/root dropdown enumeration.
  function scopeMatch(node){if(!state.scopes.size||state.scopes.has('all'))return true;return node.scopeKeys?.some(key=>state.scopes.has(key));}
  function applyFilters(resetList=true){
    if(resetList)state.listPage=0;const types=enabledEdges(),degree=new Map();
    for(const e of state.edges)if(types.has(e.kind)){degree.set(e.source,(degree.get(e.source)||0)+1);degree.set(e.target,(degree.get(e.target)||0)+1);}
    const search=$('search').value.trim().toLocaleLowerCase(),tier=$('tier-filter').value,rev=$('revision-filter').value,iso=$('isolation-filter').value;
    state.visible=state.nodes.filter(n=>scopeMatch(n)&&(!search||[n.label,n.id,n.project_label,n.hierarchy_path,sourceOf(n).root_id,sourceOf(n).relative_path,n.description,n.preview,...(n.memberships||[]).map(m=>m.project_label)].some(v=>String(v||'').toLocaleLowerCase().includes(search)))&&(!tier||n.tier===tier)&&(!rev||n.revision_state===rev)&&(!iso||(iso==='isolated')===!(degree.get(n.id)>0))&&(!state.focused||state.focused.has(n.id)));
    const ids=new Set(state.visible.map(n=>n.id)),scopeProjects=new Set([...state.scopes].map(key=>state.treeMap.get(key)?.project).filter(Boolean));state.visibleEdges=state.edges.filter(e=>types.has(e.kind)&&ids.has(e.source)&&ids.has(e.target)&&(!scopeProjects.size||scopeProjects.has(e.nucleus_id)));
    state.groupConnections=new Map();for(const e of state.visibleEdges)state.groupConnections.set(e.nucleus_id,(state.groupConnections.get(e.nucleus_id)||0)+1);
    state.pickOrder=state.visible.map(n=>({node:n,p:displayPosition(n.id)})).filter(v=>v.p).sort((a,b)=>a.p.x-b.p.x);
    state.dirty=true;$('empty-state').hidden=!!state.visible.length;text('empty-state',state.nodes.length?t('filterEmpty'):state.loading?t('loading'):t('empty'));
    const tiers=new Map();for(const n of state.visible)tiers.set(n.tier||'unclassified',(tiers.get(n.tier||'unclassified')||0)+1);
    text('tier-counts',t('tierCounts')+': '+[...tiers].map(([k,n])=>(k==='unclassified'?t('unclassified'):k)+' '+n.toLocaleString()).join(' · '));
    renderList();draw();renderHeader();
  }
  function resetFilters(apply=true){
    for(const id of ['root-filter','document-filter','tier-filter','revision-filter','isolation-filter'])$(id).value='';$('kind-filter').value='chunk';$('search').value='';
    for(const i of document.querySelectorAll('fieldset input'))i.checked=true;state.focused=null;state.scopes.clear();if(state.project===ALL_PROJECTS)$('projects').value=ALL_PROJECTS;$('unfocus').disabled=true;if(apply){renderTree();applyFilters();fitGraph();}
  }
  function renderList(){
    const list=$('node-list');list.replaceChildren();const max=Math.max(0,Math.ceil(state.visible.length/LIST_SIZE)-1);state.listPage=Math.max(0,Math.min(state.listPage,max));
    for(const n of state.visible.slice(state.listPage*LIST_SIZE,(state.listPage+1)*LIST_SIZE)){
      const li=document.createElement('li'),button=document.createElement('button'),info=document.createElement('span'),title=document.createElement('strong'),meta=document.createElement('small');
      button.className='node-row';button.dataset.nodeId=n.id;button.setAttribute('aria-pressed',String(state.selected?.id===n.id));info.className='node-info';title.textContent=n.label||n.id;meta.textContent=n.project_label+' · '+(sourceOf(n).relative_path||'');info.append(title,meta);button.append(info);button.addEventListener('click',()=>selectNode(n));li.append(button);list.append(li);
    }
    $('list-previous').disabled=state.listPage===0;$('list-next').disabled=state.listPage===max;text('list-page',state.visible.length?(state.listPage*LIST_SIZE+1)+'–'+Math.min(state.visible.length,(state.listPage+1)*LIST_SIZE)+' / '+state.visible.length:'0');
  }
  function keyOf(parts){return JSON.stringify(parts);}
  function buildTree(){
    const root={key:'all',name:t('allProjects'),type:'all',count:state.nodes.length,children:new Map()},map=new Map([['all',root]]);
    for(const n of state.nodes){
      n.scopeKeys=['all'];
      for(const member of n.memberships||[n]){
        const source=member.source||{},path=member.hierarchy_path||member.origin?.relative_path||source.relative_path||'—';
        const parts=[member.nucleus_id||member.project_id,...path.split('/').filter(Boolean)];let parent=root;member.scopeKeys=['all'];
        for(let i=0;i<parts.length;i++){
          const key=keyOf(parts.slice(0,i+1));let child=parent.children.get(key);
          if(!child){child={key,name:i===0?member.project_label:parts[i],type:i===0?'project':i===parts.length-1?'file':'folder',count:0,nodeIds:new Set(),children:new Map(),project:member.nucleus_id||member.project_id,parts:parts.slice(0,i+1)};parent.children.set(key,child);map.set(key,child);}
          if(!child.nodeIds.has(n.id)){child.nodeIds.add(n.id);child.count++;}n.scopeKeys.push(key);member.scopeKeys.push(key);parent=child;
        }
        const key=keyOf([...parts,n.id]),leaf={key,name:n.label||n.native_chunk_id||short(n.id),type:'chunk',count:1,node:n,member,children:new Map(),project:member.nucleus_id||member.project_id,parts:[...parts,n.id]};parent.children.set(key,leaf);map.set(key,leaf);n.scopeKeys.push(key);member.scopeKeys.push(key);
        member.folderKey=keyOf(parts.slice(0,-1));member.folderName=parts.slice(1,-1).join('/');
      }
      const first=n.memberships?.[0]||n;n.folderKey=first.folderKey;n.folderName=first.folderName;
    }
    state.tree=root;state.treeMap=map;state.treeRows=null;
  }
  function renderTree(rebuild=true){
    const target=$('scope-tree');if(!state.tree){target.replaceChildren();return;}
    if(rebuild||!state.treeRows){
      const rows=[];
      const walk=(item,depth)=>{
        rows.push({item,depth});if(!item.children.size||!state.treeExpanded.has(item.key))return;
        const children=[...item.children.values()].sort((a,b)=>a.type==='chunk'&&b.type==='chunk'?(Number(sourceOf(a.node).start_byte||0)-Number(sourceOf(b.node).start_byte||0)||a.key.localeCompare(b.key)):a.name.localeCompare(b.name));
        const limit=(state.treePages.get(item.key)||1)*TREE_PAGE;for(const child of children.slice(0,limit))walk(child,depth+1);if(children.length>limit)rows.push({item,depth:depth+1,more:children.length-limit});
      };walk(state.tree,0);state.treeRows=rows;
    }
    const top=target.scrollTop,rowHeight=29,start=Math.max(0,Math.floor(top/rowHeight)-6),end=Math.min(state.treeRows.length,start+Math.ceil((target.clientHeight||400)/rowHeight)+14);
    const fragment=document.createDocumentFragment(),before=document.createElement('div');before.style.height=(start*rowHeight)+'px';before.setAttribute('aria-hidden','true');fragment.append(before);
    for(const entry of state.treeRows.slice(start,end)){
      const {item,depth,more}=entry;
      if(more){const button=document.createElement('button');button.className='tree-more';button.textContent=(lang==='en'?'More… ':'Más… ')+more;button.style.marginLeft=(depth*11)+'px';button.addEventListener('click',()=>{state.treePages.set(item.key,(state.treePages.get(item.key)||1)+1);renderTree();});fragment.append(button);continue;}
      const row=document.createElement('div');row.className='tree-row';row.setAttribute('role','treeitem');row.setAttribute('aria-level',String(depth+1));row.dataset.scopeKey=item.key;
      const expanded=state.treeExpanded.has(item.key);if(item.children.size)row.setAttribute('aria-expanded',String(expanded));
      const toggle=document.createElement('button');toggle.className='tree-toggle';toggle.textContent=item.children.size?(expanded?'▾':'▸'):'·';toggle.setAttribute('aria-label',(expanded?'Collapse ':'Expand ')+item.name);toggle.disabled=!item.children.size;
      toggle.addEventListener('click',()=>{if(expanded)state.treeExpanded.delete(item.key);else state.treeExpanded.add(item.key);renderTree();});
      const select=document.createElement('input');select.type='checkbox';select.checked=item.key==='all'?!state.scopes.size:state.scopes.has(item.key);select.indeterminate=item.key!=='all'&&!select.checked&&(!state.scopes.size||(item.parts||[]).some((_,i)=>state.scopes.has(keyOf(item.parts.slice(0,i+1)))));select.setAttribute('aria-label',item.name+' · '+item.count+' chunks');
      select.addEventListener('change',()=>{if(item.key==='all')state.scopes.clear();else if(select.checked){state.scopes.delete('all');for(let i=1;i<(item.parts||[]).length;i++)state.scopes.delete(keyOf(item.parts.slice(0,i)));state.scopes.add(item.key);}else state.scopes.delete(item.key);applyFilters();renderTree();fitGraph();});
      const button=document.createElement('button');button.className='tree-name';button.textContent=item.name;button.title=item.name;button.dataset.scopeKey=item.key;
      button.addEventListener('click',event=>{if(item.type==='chunk'){selectNode(item.node,item.member);return;}if(event.ctrlKey||event.metaKey||event.shiftKey){if(state.scopes.has(item.key))state.scopes.delete(item.key);else state.scopes.add(item.key);}else{state.scopes.clear();if(item.key!=='all')state.scopes.add(item.key);}state.treeExpanded.add(item.key);applyFilters();renderTree();fitGraph();});
      const swatch=document.createElement('span');swatch.className='color-swatch';swatch.style.backgroundColor=(item.type==='project'?projectColor(item.project):folderColor(item.project,item.type==='chunk'?item.member.folderKey:item.type==='file'?keyOf(item.parts.slice(0,-1)):item.key)).css;
      const count=document.createElement('span');count.className='tree-count';count.textContent=item.count.toLocaleString();row.style.paddingLeft=(depth*11)+'px';row.append(toggle,select,swatch,button,count);fragment.append(row);
    }
    const after=document.createElement('div');after.style.height=((state.treeRows.length-end)*rowHeight)+'px';after.setAttribute('aria-hidden','true');fragment.append(after);target.replaceChildren(fragment);target.scrollTop=top;
  }
  function hash(value){value=String(value??'');let h=2166136261;for(let i=0;i<value.length;i++){h^=value.charCodeAt(i);h=Math.imul(h,16777619);}return h>>>0;}
  function hsl(h,s,l){
    const a=s*Math.min(l,1-l),f=n=>{const k=(n+h/30)%12;return l-a*Math.max(-1,Math.min(k-3,9-k,1));},rgb=[f(0),f(8),f(4)];
    return {rgb,css:'rgb('+rgb.map(v=>Math.round(v*255)).join(',')+')'};
  }
  const colorCache=new Map();
  function projectColor(id){return window.VrgNuclei.color(id);}
  function folderColor(project,folder){return window.VrgNuclei.folderColor(project,folder);}
  function nodeColor(n){if($('color-mode').value==='tier')return ({HOT:{rgb:[1,.35,.4],css:'#ff5966'},WARM:{rgb:[1,.74,.3],css:'#ffbd4d'},COLD:{rgb:[.33,.58,1],css:'#5494ff'}})[n.tier]||{rgb:[.5,.59,.7],css:'#8096b3'};return $('color-mode').value==='folder'?folderColor(nucleusOf(n),n.folderKey):projectColor(nucleusOf(n));}
  function renderProjects(){
    const legend=$('project-legend');legend.replaceChildren();legend.hidden=false;
    for(const item of state.tree?.children.values()||[]){const button=document.createElement('button'),dot=document.createElement('span');dot.className='color-swatch';dot.style.backgroundColor=projectColor(item.project).css;button.append(dot,document.createTextNode(item.name));button.addEventListener('click',()=>{state.scopes=new Set([item.key]);applyFilters();renderTree();fitGraph();});legend.append(button);}
    const folders=$('folder-legend');folders.replaceChildren();let count=0;
    // Folder colors are also visible beside every lazy tree entry; the compact legend is paged.
    const entries=[...state.treeMap.values()].filter(v=>v.type==='folder').sort((a,b)=>a.key.localeCompare(b.key));
    const page=Number(folders.dataset.page||0),start=page*50;
    for(const item of entries.slice(start,start+50)){const row=document.createElement('span'),dot=document.createElement('span');dot.className='color-swatch';dot.style.backgroundColor=folderColor(item.project,item.key).css;row.title=projectName(item.project)+' / '+item.parts.slice(1).join('/');row.append(dot,document.createTextNode(item.name));folders.append(row);count++;}
    if(entries.length>50){const prev=document.createElement('button'),next=document.createElement('button');prev.textContent='‹';next.textContent='›';prev.disabled=page===0;next.disabled=start+50>=entries.length;prev.addEventListener('click',()=>{folders.dataset.page=String(page-1);renderProjects();});next.addEventListener('click',()=>{folders.dataset.page=String(page+1);renderProjects();});folders.append(prev,document.createTextNode((start+1)+'–'+(start+count)+' / '+entries.length),next);}
  }
  function layout(){
    if(!window.VrgNuclei)return;
    state.nuclei=window.VrgNuclei.build(state.nodes,visualProjects(),{separation:Number($('nucleus-separation').value),density:Number($('nucleus-density').value)});
    state.positions=state.nuclei.positions;let digest=2166136261;for(const [id,p]of [...state.positions].sort((a,b)=>a[0].localeCompare(b[0]))){digest=hash(digest+':'+id+':'+p.x.toFixed(6)+':'+p.y.toFixed(6));}state.positionHash=digest.toString(16);state.dirty=true;state.lineKey=null;state.lodKey=null;
  }
  function currentFolder(){const g=state.nuclei?.projects.find(g=>g.id===state.navigation.projectId);return g?.folders.find(f=>f.key===state.navigation.folderKey);}
  function prepareFolderView(){
    state.folderPositions=new Map();const f=currentFolder();if(f){for(const id of f.nodeIds){const p=state.positions.get(id);if(!p)continue;const path=JSON.parse(p.folderKey||'[]').slice(1).join('/');if(path===f.path||path.startsWith(f.path+'/'))continue;const angle=hash('member-angle:'+id+f.key)/4294967296*Math.PI*2,r=Math.sqrt((hash('member-radius:'+id+f.key)+.5)/4294967296)*f.radius*.84;state.folderPositions.set(id,{x:f.x+Math.cos(angle)*r,y:f.y+Math.sin(angle)*r,projectId:f.projectId,folderKey:f.key});}}
    state.pickOrder=state.visible.map(node=>({node,p:displayPosition(node.id)})).filter(v=>v.p).sort((a,b)=>a.p.x-b.p.x);
  }
  function displayPosition(id){return state.folderPositions?.get(id)||state.positions.get(id);}
  function folderMember(node){const f=currentFolder();if(!f)return null;return node.memberships?.find(m=>{const path=m.hierarchy_path||m.origin?.relative_path||'';return path.startsWith(f.path+'/');})||null;}
  function bounds(nodes){let minX=Infinity,minY=Infinity,maxX=-Infinity,maxY=-Infinity;for(const n of nodes){const p=state.positions.get(n.id);if(!p)continue;minX=Math.min(minX,p.x);minY=Math.min(minY,p.y);maxX=Math.max(maxX,p.x);maxY=Math.max(maxY,p.y);}return minX===Infinity?{minX:-100,minY:-100,maxX:100,maxY:100}:{minX,minY,maxX,maxY};}
  function circleBounds(g){const r=Math.max(g.radius,2)*1.16;return {minX:g.x-r,maxX:g.x+r,minY:g.y-r,maxY:g.y+r};}
  function allBounds(){const groups=state.nuclei?.projects||[];if(!groups.length)return bounds(state.nodes);return {minX:Math.min(...groups.map(g=>g.x-g.radius*1.22)),maxX:Math.max(...groups.map(g=>g.x+g.radius*1.22)),minY:Math.min(...groups.map(g=>g.y-g.radius*1.22)),maxY:Math.max(...groups.map(g=>g.y+g.radius*1.22))};}
  function cameraFor(b){const w=canvas.clientWidth||500,h=canvas.clientHeight||400,scale=Math.max(.00001,Math.min((w-100)/Math.max(1e-5,b.maxX-b.minX),(h-100)/Math.max(1e-5,b.maxY-b.minY)));return {scale,panX:w/2-(b.minX+b.maxX)*.5*scale,panY:h/2-(b.minY+b.maxY)*.5*scale};}
  function cameraTo(b,animate=false){
    const target=cameraFor(b);if(state.cameraAnimation)cancelAnimationFrame(state.cameraAnimation);
    if(!animate||matchMedia('(prefers-reduced-motion: reduce)').matches){Object.assign(state,target);draw();return;}
    const from={scale:state.scale,panX:state.panX,panY:state.panY},start=performance.now();
    const frame=now=>{const u=Math.min(1,(now-start)/420),v=1-Math.pow(1-u,3);for(const key of ['scale','panX','panY'])state[key]=from[key]+(target[key]-from[key])*v;draw();state.cameraAnimation=u<1?requestAnimationFrame(frame):0;};state.cameraAnimation=requestAnimationFrame(frame);
  }
  function fitGraph(){
    state.navigation={projectId:null,folderKey:null};prepareFolderView();$('back-nucleus').disabled=true;
    const b=state.scopes.size||state.focused?bounds(state.visible):allBounds();cameraTo(b);state.fitScale=state.scale;state.overviewScale=cameraFor(allBounds()).scale;draw();
  }
  function focusNucleus(g){state.navigation={projectId:g.id,folderKey:null};prepareFolderView();state.dirty=true;state.lineKey=null;cameraTo(circleBounds(g),true);$('back-nucleus').disabled=false;hideTooltip();draw();}
  function focusFolder(g,f){state.navigation={projectId:g.id,folderKey:f.key};prepareFolderView();state.dirty=true;state.lineKey=null;cameraTo(circleBounds(f),true);$('back-nucleus').disabled=false;hideTooltip();draw();}
  function goBack(){const g=state.nuclei?.projects.find(g=>g.id===state.navigation.projectId);if(state.navigation.folderKey&&g)focusNucleus(g);else fitGraph();}
  function lodLevel(){if(state.navigation.folderKey)return 'chunks';if(state.navigation.projectId)return 'folders';const relative=state.scale/Math.max(1e-8,state.overviewScale||state.fitScale);return relative>7?'chunks':relative>2.1?'folders':'projects';}
  function activeNucleus(){if(state.navigation.projectId)return state.nuclei?.projects.find(g=>g.id===state.navigation.projectId);if(lodLevel()==='projects')return null;const x=(canvas.clientWidth/2-state.panX)/state.scale,y=(canvas.clientHeight/2-state.panY)/state.scale;return (state.nuclei?.projects||[]).reduce((a,g)=>!a||Math.hypot(g.x-x,g.y-y)<Math.hypot(a.x-x,a.y-y)?g:a,null);}
  function levelFolders(g){if(!g)return [];const nav=state.navigation.folderKey,parent=nav?g.folders.find(f=>f.key===nav):null;const list=g.folders.filter(f=>!parent||f.path===parent.path||f.path.startsWith(parent.path+'/'));const levels=list.filter(f=>f.path&&f.path.split('/').filter(Boolean).length===(parent?parent.path.split('/').filter(Boolean).length+1:1));return levels.length?levels:list.filter(f=>f.path==='').slice(0,1);}
  let gl=null,gpu=null,contextLost=false;
  const vertex=`#version 300 es
  precision highp float;
  layout(location=0) in vec2 a_position;
  layout(location=1) in vec3 a_color;
  uniform vec2 u_resolution;
  uniform vec2 u_pan;
  uniform float u_scale;
  uniform float u_size;
  out vec3 v_color;
  void main(){vec2 p=(a_position*u_scale+u_pan)/u_resolution;gl_Position=vec4(p*vec2(2.,-2.)+vec2(-1.,1.),0.,1.);gl_PointSize=u_size;v_color=a_color;}`;
  function initGpu(){
    try{
      gl=canvas.getContext('webgl2',{alpha:true,antialias:false,preserveDrawingBuffer:false,powerPreference:'high-performance'});if(!gl)return;
      const makeProgram=points=>{
        const shader=(kind,source)=>{const s=gl.createShader(kind);gl.shaderSource(s,source);gl.compileShader(s);if(!gl.getShaderParameter(s,gl.COMPILE_STATUS))throw new Error('GPU_SHADER');return s;};
        const p=gl.createProgram(),v=shader(gl.VERTEX_SHADER,vertex),f=shader(gl.FRAGMENT_SHADER,`#version 300 es\nprecision highp float;in vec3 v_color;uniform float u_alpha;out vec4 outputColor;void main(){${points?'float d=distance(gl_PointCoord,vec2(.5));if(d>.5)discard;':''}outputColor=vec4(v_color,u_alpha);}`);
        gl.attachShader(p,v);gl.attachShader(p,f);gl.linkProgram(p);gl.deleteShader(v);gl.deleteShader(f);if(!gl.getProgramParameter(p,gl.LINK_STATUS))throw new Error('GPU_PROGRAM');
        return {program:p,resolution:gl.getUniformLocation(p,'u_resolution'),pan:gl.getUniformLocation(p,'u_pan'),scale:gl.getUniformLocation(p,'u_scale'),size:gl.getUniformLocation(p,'u_size'),alpha:gl.getUniformLocation(p,'u_alpha')};
      };
      gpu={points:makeProgram(true),lines:makeProgram(false),pointBuffer:null,lineBuffers:[],selection:null};state.dirty=true;
    }catch(_){gl=null;gpu=null;message('error',t('webgl'));}
  }
  function makeBuffer(values){const vao=gl.createVertexArray(),buffer=gl.createBuffer();gl.bindVertexArray(vao);gl.bindBuffer(gl.ARRAY_BUFFER,buffer);gl.bufferData(gl.ARRAY_BUFFER,values,gl.STATIC_DRAW);gl.enableVertexAttribArray(0);gl.vertexAttribPointer(0,2,gl.FLOAT,false,20,0);gl.enableVertexAttribArray(1);gl.vertexAttribPointer(1,3,gl.FLOAT,false,20,8);return {vao,buffer,count:values.length/5};}
  function deleteBuffer(buffer){if(buffer){gl.deleteVertexArray(buffer.vao);gl.deleteBuffer(buffer.buffer);}}
  function uploadGpu(){
    if(!gpu||!gl)return;deleteBuffer(gpu.pointBuffer);if(gpu.indices)gl.deleteBuffer(gpu.indices);gpu.indices=gl.createBuffer();
    const points=new Float32Array(state.visible.length*5);let at=0;const active=activeNucleus();state.gpuNodes=[];
    for(const n of state.visible){const p=displayPosition(n.id);if(!p)continue;const c=nodeColor(n).rgb,dim=active&&nucleusOf(n)!==active.id ? .16 : 1;points[at++]=p.x;points[at++]=p.y;points[at++]=c[0]*dim;points[at++]=c[1]*dim;points[at++]=c[2]*dim;state.gpuNodes.push(n);}
    gpu.pointBuffer=makeBuffer(points.subarray(0,at));state.gpuResident=state.gpuNodes.length;state.lodKey=null;state.lineKey=null;
    state.dirty=false;
  }
  function updateLod(){
    const level=lodLevel(),key=level+':'+(state.navigation.folderKey||'')+':'+(activeNucleus()?.id||'');if(state.lodKey===key)return;state.lodKey=key;const active=activeNucleus(),indices=[],buckets=new Map();
    state.gpuNodes.forEach((n,i)=>{if(!buckets.has(nucleusOf(n)))buckets.set(nucleusOf(n),[]);buckets.get(nucleusOf(n)).push({n,i,rank:hash(n.id)});});
    for(const [id,bucket]of buckets){bucket.sort((a,b)=>a.rank-b.rank);const limit=level==='chunks'?bucket.length:level==='folders'&&id===active?.id?Math.min(bucket.length,2800):Math.min(bucket.length,Math.round(150+Math.log2(bucket.length+1)*28));for(let k=0;k<limit;k++)indices.push(bucket[k].i);}
    gl.bindVertexArray(gpu.pointBuffer.vao);gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER,gpu.indices);gl.bufferData(gl.ELEMENT_ARRAY_BUFFER,new Uint32Array(indices),gl.STATIC_DRAW);state.drawnIndices=indices;state.drawnIds=new Set(indices.map(i=>state.gpuNodes[i].id));
  }
  function updateLines(){
    const level=lodLevel(),active=activeNucleus(),detail=Number($('edge-detail').value)/100,key=[state.lodKey,detail,state.selected?.id||'',state.visibleEdges.length].join(':');if(key===state.lineKey)return;state.lineKey=key;
    for(const b of gpu.lineBuffers)deleteBuffer(b);gpu.lineBuffers=[];const limit=level==='projects'?Math.floor(700*detail):level==='folders'?Math.floor(5500*detail):detail===1?Infinity:Math.floor(35000*detail),data=[];let used=0;
    const stride=Math.max(1,Math.ceil(state.visibleEdges.length/Math.max(1,limit)));
    for(const e of state.visibleEdges){
      const a=state.nodeMap.get(e.source),b=state.nodeMap.get(e.target);if(!a||!b)continue;const selected=state.selected&&(e.source===state.selected.id||e.target===state.selected.id);
      if(!selected&&(active&&nucleusOf(a)!==active.id||used>=limit||(e.rank??hash(e.id))%stride!==0))continue;
      const pa=displayPosition(e.source),pb=displayPosition(e.target);if(!pa||!pb)continue;const ca=projectColor(nucleusOf(a)).rgb,cb=projectColor(nucleusOf(b)).rgb;
      if(nucleusOf(a)===nucleusOf(b)){for(const p of[pa,pb])data.push(p.x,p.y,...ca);}
      else {const dx=pb.x-pa.x,dy=pb.y-pa.y,cx=(pa.x+pb.x)/2-dy*.13,cy=(pa.y+pb.y)/2+dx*.13;let prev=pa;for(let j=1;j<=12;j++){const t=j/12,p={x:(1-t)*(1-t)*pa.x+2*(1-t)*t*cx+t*t*pb.x,y:(1-t)*(1-t)*pa.y+2*(1-t)*t*cy+t*t*pb.y};for(const v of[prev,p])data.push(v.x,v.y,...ca.map((x,i)=>x*(1-t)+cb[i]*t));prev=p;}}
      used++;
    }
    if(data.length)gpu.lineBuffers.push(makeBuffer(new Float32Array(data)));state.bufferedEdges=used;
  }
  function draw(){if(state.frame)return;state.frame=requestAnimationFrame(paint);}
  function paint(){
    state.frame=0;if(!gpu||!gl||contextLost){state.rendered=0;text('visible-count',t('webgl'));drawOverlay();return;}
    const colorFocus=activeNucleus()?.id||null;if(colorFocus!==state.colorFocus){state.colorFocus=colorFocus;state.dirty=true;}if(state.dirty)uploadGpu();updateLod();updateLines();gl.viewport(0,0,canvas.width,canvas.height);gl.clearColor(0,0,0,0);gl.clear(gl.COLOR_BUFFER_BIT);gl.enable(gl.BLEND);gl.blendFunc(gl.SRC_ALPHA,gl.ONE_MINUS_SRC_ALPHA);
    const ratio=devicePixelRatio||1,w=canvas.clientWidth||1,h=canvas.clientHeight||1;
    const use=(program,size,alpha)=>{gl.useProgram(program.program);gl.uniform2f(program.resolution,w,h);gl.uniform2f(program.pan,state.panX,state.panY);gl.uniform1f(program.scale,state.scale);gl.uniform1f(program.size,size*ratio);gl.uniform1f(program.alpha,alpha);};
    const level=lodLevel(),opacity=Number($('edge-opacity').value)/100;
    const drawLines=opacity>0&&state.scale/Math.max(.00001,state.overviewScale||state.fitScale)>=Number($('edge-zoom').value);state.renderedEdges=drawLines?(state.bufferedEdges||0):0;if(drawLines){use(gpu.lines,1,opacity*(level==='projects'?.30:.6));for(const b of gpu.lineBuffers){gl.bindVertexArray(b.vao);gl.drawArrays(gl.LINES,0,b.count);}}
    const size=level==='projects'?1.55:level==='folders'?2:3.4;use(gpu.points,size,.86);gl.bindVertexArray(gpu.pointBuffer.vao);gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER,gpu.indices);gl.drawElements(gl.POINTS,state.drawnIndices.length,gl.UNSIGNED_INT,0);state.rendered=state.drawnIndices.length;
    if(state.selected&&state.drawnIds.has(state.selected.id)){
      const p=displayPosition(state.selected.id);if(p){deleteBuffer(gpu.selection);gpu.selection=makeBuffer(new Float32Array([p.x,p.y,...nodeColor(state.selected).rgb]));use(gpu.points,14,.18);gl.bindVertexArray(gpu.selection.vao);gl.drawArrays(gl.POINTS,0,1);use(gpu.points,5,1);gl.drawArrays(gl.POINTS,0,1);}
    }
    state.inViewport=0;for(const i of state.drawnIndices){const p=displayPosition(state.gpuNodes[i].id),x=p.x*state.scale+state.panX,y=p.y*state.scale+state.panY;if(x>=0&&x<=w&&y>=0&&y<=h)state.inViewport++;}
    text('visible-count',(lang==='en'?'LOD ':'Nivel ')+({projects:lang==='en'?'projects':'proyectos',folders:lang==='en'?'folders':'carpetas',chunks:'chunks'})[level]+' · '+state.inViewport.toLocaleString()+' '+t('visible'));
    canvas.dataset.renderedNodes=String(state.rendered);canvas.dataset.gpuResidentNodes=String(state.gpuResident);canvas.dataset.loadedNodes=String(state.nodes.length);canvas.dataset.renderedEdges=String(state.renderedEdges);canvas.dataset.renderer='WebGL2';canvas.dataset.lod=level;drawOverlay();renderHeader();
  }
  function resize(){const ratio=devicePixelRatio||1;canvas.width=Math.round(canvas.clientWidth*ratio);canvas.height=Math.round(canvas.clientHeight*ratio);const overlay=$('graph-overlay');overlay.width=canvas.width;overlay.height=canvas.height;if(state.nuclei)state.overviewScale=cameraFor(allBounds()).scale;draw();}
  function zoom(factor,x=canvas.clientWidth/2,y=canvas.clientHeight/2){const next=Math.min(1e7,Math.max(.00001,state.scale*factor)),ratio=next/state.scale;state.panX=x-(x-state.panX)*ratio;state.panY=y-(y-state.panY)*ratio;state.scale=next;hideTooltip();draw();}
  function pick(x,y){
    const list=state.pickOrder||[],wx=(x-state.panX)/state.scale,wy=(y-state.panY)/state.scale,radius=9/state.scale;let lo=0,hi=list.length;
    while(lo<hi){const mid=(lo+hi)>>>1;if(list[mid].p.x<wx-radius)lo=mid+1;else hi=mid;}
    let selected=null,best=radius*radius;for(let i=lo;i<list.length&&list[i].p.x<=wx+radius;i++){const v=list[i];if(state.drawnIds&&!state.drawnIds.has(v.node.id))continue;const dx=v.p.x-wx,dy=v.p.y-wy,d=dx*dx+dy*dy;if(d<best){best=d;selected=v.node;}}return selected;
  }
  let hoverFrame=0,hoverNode=null,hoverRequest=0,hoverTimer;
  function hideTooltip(){hoverNode=null;hoverRequest++;clearTimeout(hoverTimer);$('graph-tooltip').hidden=true;}
  function showTooltip(node,x,y){
    const tip=$('graph-tooltip');if(hoverNode?.id===node.id){tip.style.left=Math.max(8,Math.min(x+14,canvas.clientWidth-330))+'px';tip.style.top=Math.max(8,Math.min(y+14,canvas.clientHeight-155))+'px';return;}tip.replaceChildren();const name=document.createElement('strong'),info=document.createElement('span'),id=document.createElement('small'),description=document.createElement('span');
    name.textContent=node.label||node.chunk_id||short(node.id);info.textContent=[...new Set((node.memberships||[node]).map(m=>m.project_label))].join(', ')+' / '+(node.folderName||'')+'\n'+(node.hierarchy_path||node.origin?.relative_path||sourceOf(node).relative_path||'');id.textContent=node.native_chunk_id||node.chunk_id||node.native_id||node.id;description.className='tooltip-description';description.textContent=String(node.description||node.preview||node.excerpt||'').slice(0,200);tip.append(name,info,id,description);
    tip.style.left=Math.max(8,Math.min(x+14,canvas.clientWidth-330))+'px';tip.style.top=Math.max(8,Math.min(y+14,canvas.clientHeight-155))+'px';tip.hidden=false;
    if(hoverNode?.id===node.id)return;hoverNode=node;const request=++hoverRequest;clearTimeout(hoverTimer);
    if(!description.textContent)hoverTimer=setTimeout(async()=>{
      const owner=node.project_id,snapshot=state.data?.snapshots?.[owner]?.snapshot_id;if(!snapshot)return;
      try{const q=new URLSearchParams({snapshot_id:snapshot,offset:'0',limit:'256'}),detail=await api(projectEndpoint(owner,'nodes/'+encodeURIComponent(node.native_id))+'?'+q);if(request!==hoverRequest||hoverNode?.id!==node.id)return;node.preview=String(detail.text||'').replace(/\s+/g,' ').slice(0,200);description.textContent=node.preview;}catch(error){if(request===hoverRequest&&['UNAUTHORIZED','ROOT_REVOKED','POLICY_CHANGED','SNAPSHOT_EXPIRED','SOURCE_CHANGED'].includes(errorCode(error)))showError(error);}
    },180);
  }
  function labelOnCanvas(ctx,label,x,y,type,owner){
    const w=canvas.clientWidth,h=canvas.clientHeight,font=type==='project'?12:10;ctx.font=(type==='project'?'600 ':'500 ')+font+'px system-ui, sans-serif';
    const width=Math.min(w-16,ctx.measureText(label).width+20),height=type==='project'?26:22;
    const anchors=[[x,y],[x,y+30],[x,y-30],[x,y+58],[x,y-58],[x-40,y],[x+40,y]];let box;
    for(const [cx,cy]of anchors){const next={x:Math.max(8,Math.min(w-width-8,cx-width/2)),y:Math.max(8,Math.min(h-height-8,cy-height/2)),w:width,h:height};if(!state.overlayLabels.some(b=>next.x<b.x+b.w+5&&next.x+next.w+5>b.x&&next.y<b.y+b.h+4&&next.y+next.h+4>b.y)){box=next;break;}}
    if(!box)return;const color=owner.color?.css||projectColor(owner.projectId||owner.id).css;
    ctx.fillStyle='rgba(6,14,30,.86)';ctx.beginPath();ctx.roundRect(box.x,box.y,box.w,box.h,7);ctx.fill();
    ctx.fillStyle=type==='project'?'#e6f1ff':color;ctx.textAlign='center';ctx.textBaseline='middle';ctx.shadowColor=color;ctx.shadowBlur=type==='project'?8:0;ctx.fillText(label,box.x+box.w/2,box.y+box.h/2,box.w-12);ctx.shadowBlur=0;
    state.overlayLabels.push({...box,text:label,type,projectId:owner.projectId||owner.id,key:owner.key});
  }
  function drawOverlay(){
    const overlay=$('graph-overlay'),ctx=overlay?.getContext('2d');if(!ctx)return;const dpr=devicePixelRatio||1,w=canvas.clientWidth,h=canvas.clientHeight;ctx.setTransform(dpr,0,0,dpr,0,0);ctx.clearRect(0,0,w,h);state.overlayLabels=[];
    const groups=state.nuclei?.projects||[],level=lodLevel(),active=activeNucleus();
    for(const g of groups){
      const x=g.x*state.scale+state.panX,y=g.y*state.scale+state.panY,r=g.radius*state.scale;if(x+r<0||x-r>w||y+r<0||y-r>h)continue;
      const c=g.color.rgb.map(v=>Math.round(v*255)),dim=active&&active.id!==g.id ? .25 : 1;
      if(level==='projects'){
        const halo=ctx.createRadialGradient(x,y,0,x,y,Math.max(1,r*1.3));halo.addColorStop(0,'rgba('+c.join(',')+','+(.075*dim)+')');halo.addColorStop(1,'rgba('+c.join(',')+',0)');ctx.fillStyle=halo;ctx.fillRect(x-r*1.3,y-r*1.3,r*2.6,r*2.6);
        ctx.strokeStyle='rgba('+c.join(',')+','+(.12*dim)+')';ctx.lineWidth=.7;ctx.beginPath();ctx.arc(x,y,r*1.04,0,Math.PI*2);ctx.stroke();
        labelOnCanvas(ctx,g.label,x,y+r+19,'project',g);
      }else if(!active||g.id===active.id){
        const folders=levelFolders(g);
        for(const f of folders){const fx=f.x*state.scale+state.panX,fy=f.y*state.scale+state.panY,fr=f.radius*state.scale;if(fr<9||fx+fr<0||fx-fr>w||fy+fr<0||fy-fr>h)continue;
          ctx.strokeStyle='rgba('+c.join(',')+',.18)';ctx.lineWidth=.7;ctx.beginPath();ctx.arc(fx,fy,fr,0,Math.PI*2);ctx.stroke();if(fr>=18)labelOnCanvas(ctx,f.label,fx,fy+fr+10,'folder',f);
        }
      }
    }
    text('navigation-breadcrumb',state.navigation.projectId?[projectName(state.navigation.projectId),active?.folders.find(f=>f.key===state.navigation.folderKey)?.path].filter(Boolean).join('  /  '):t('allProjects'));
    $('back-nucleus').disabled=!state.navigation.projectId;drawMinimap();
  }
  function drawMinimap(){
    const mini=$('minimap'),ctx=mini?.getContext('2d');if(!ctx||!state.nuclei)return;const ratio=devicePixelRatio||1,w=mini.clientWidth||150,h=mini.clientHeight||100;if(mini.width!==Math.round(w*ratio)||mini.height!==Math.round(h*ratio)){mini.width=Math.round(w*ratio);mini.height=Math.round(h*ratio);}ctx.setTransform(ratio,0,0,ratio,0,0);ctx.clearRect(0,0,w,h);
    const b=allBounds(),scale=Math.min((w-12)/(b.maxX-b.minX),(h-12)/(b.maxY-b.minY)),ox=(w-(b.maxX-b.minX)*scale)/2,oy=(h-(b.maxY-b.minY)*scale)/2;state.miniTransform={b,scale,ox,oy};
    for(const g of state.nuclei.projects){ctx.fillStyle=g.color.css;ctx.globalAlpha=.65;ctx.beginPath();ctx.arc(ox+(g.x-b.minX)*scale,oy+(g.y-b.minY)*scale,Math.max(2,g.radius*scale),0,Math.PI*2);ctx.fill();}
    ctx.globalAlpha=1;ctx.strokeStyle='#b9d9fa';ctx.lineWidth=1;const left=(-state.panX/state.scale-b.minX)*scale+ox,top=(-state.panY/state.scale-b.minY)*scale+oy;ctx.strokeRect(left,top,canvas.clientWidth/state.scale*scale,canvas.clientHeight/state.scale*scale);
  }
  function clusterAt(x,y){
    const label=state.overlayLabels.find(b=>x>=b.x&&x<=b.x+b.w&&y>=b.y&&y<=b.y+b.h);if(label){const group=state.nuclei.projects.find(g=>g.id===label.projectId);if(group)return {kind:label.type,group,folder:group.folders.find(f=>f.key===label.key)};}
    const level=lodLevel(),wx=(x-state.panX)/state.scale,wy=(y-state.panY)/state.scale;
    if(level==='projects'){const group=(state.nuclei?.projects||[]).find(g=>Math.hypot(wx-g.x,wy-g.y)<=g.radius*1.18);return group?{kind:'project',group}:null;}
    if(level==='folders'){const group=activeNucleus(),folder=levelFolders(group).filter(f=>Math.hypot(wx-f.x,wy-f.y)<=f.radius).sort((a,b)=>a.radius-b.radius)[0];return folder?{kind:'folder',group,folder}:null;}
    return null;
  }
  function showClusterTooltip(hit,x,y){
    const g=hit.group,f=hit.folder,tip=$('graph-tooltip'),key=f?.key||g.id;if(hoverNode?.clusterKey===key){tip.style.left=Math.max(8,Math.min(x+14,canvas.clientWidth-300))+'px';tip.style.top=Math.max(8,Math.min(y+14,canvas.clientHeight-140))+'px';return;}
    hideTooltip();hoverNode={clusterKey:key};tip.replaceChildren();const name=document.createElement('strong'),info=document.createElement('span');name.textContent=f?f.label:g.label;name.style.color=g.color.css;
    const ids=f?.nodeIds||g.nodeIds,folders=f?g.folders.filter(v=>v.parentKey===f.key).length:g.folders.length;
    info.textContent=ids.length.toLocaleString()+' chunks · '+folders.toLocaleString()+' '+(lang==='en'?'folders':'carpetas')+(f?'\n'+f.path:'\n'+(state.groupConnections?.get(g.id)||0).toLocaleString()+' '+(lang==='en'?'real relations':'relaciones reales'));
    const hint=document.createElement('small');hint.textContent=lang==='en'?'Click to explore':'Haz clic para explorar';tip.append(name,info,hint);tip.style.left=Math.max(8,Math.min(x+14,canvas.clientWidth-300))+'px';tip.style.top=Math.max(8,Math.min(y+14,canvas.clientHeight-140))+'px';tip.hidden=false;
  }
  let drag=null;
  canvas.addEventListener('pointerdown',event=>{hideTooltip();drag={x:event.clientX,y:event.clientY,panX:state.panX,panY:state.panY,moved:false};canvas.setPointerCapture(event.pointerId);});
  canvas.addEventListener('pointermove',event=>{
    if(drag){const dx=event.clientX-drag.x,dy=event.clientY-drag.y;drag.moved=drag.moved||Math.hypot(dx,dy)>4;state.panX=drag.panX+dx;state.panY=drag.panY+dy;draw();return;}
    if(hoverFrame)cancelAnimationFrame(hoverFrame);hoverFrame=requestAnimationFrame(()=>{hoverFrame=0;const rect=canvas.getBoundingClientRect(),x=event.clientX-rect.left,y=event.clientY-rect.top,hit=clusterAt(x,y),node=hit?null:lodLevel()==='chunks'?pick(x,y):null;if(hit)showClusterTooltip(hit,x,y);else if(node)showTooltip(node,x,y);else hideTooltip();});
  });
  canvas.addEventListener('pointerup',event=>{if(!drag)return;const moved=drag.moved;drag=null;if(moved)return;const rect=canvas.getBoundingClientRect(),x=event.clientX-rect.left,y=event.clientY-rect.top,hit=clusterAt(x,y);if(hit?.kind==='project')focusNucleus(hit.group);else if(hit?.folder)focusFolder(hit.group,hit.folder);else if(lodLevel()==='chunks'){const n=pick(x,y);if(n)selectNode(n);}});
  canvas.addEventListener('pointercancel',()=>{drag=null;});canvas.addEventListener('pointerleave',hideTooltip);
  canvas.addEventListener('wheel',event=>{event.preventDefault();const rect=canvas.getBoundingClientRect();zoom(Math.exp(-event.deltaY*.0015),event.clientX-rect.left,event.clientY-rect.top);},{passive:false});
  canvas.addEventListener('keydown',event=>{if(event.key==='+'||event.key==='=')zoom(1.2);else if(event.key==='-')zoom(1/1.2);else if(event.key==='0')fitGraph();else if(['ArrowLeft','ArrowRight','ArrowUp','ArrowDown'].includes(event.key)){state.panX+=event.key==='ArrowLeft'?30:event.key==='ArrowRight'?-30:0;state.panY+=event.key==='ArrowUp'?30:event.key==='ArrowDown'?-30:0;draw();}else return;event.preventDefault();});
  canvas.addEventListener('webglcontextlost',event=>{event.preventDefault();contextLost=true;state.rendered=0;message('error',t('webgl'));renderHeader();});
  canvas.addEventListener('webglcontextrestored',()=>{contextLost=false;initGpu();draw();});
  function metadataLine(label,value) { const dt = document.createElement('dt'); dt.textContent = label; const dd = document.createElement('dd'); dd.textContent = scalar(value); $('metadata').append(dt,dd); return dd; }
  function provenanceLines(provenance) {
    if (!provenance || typeof provenance!=='object') return;
    const sources=Array.isArray(provenance.sources)?provenance.sources:[];
    if (sources.length) {
      const dt=document.createElement('dt');dt.textContent=t('provenance');const dd=document.createElement('dd');
      const list=document.createElement('ul');list.className='provenance-sources';
      for (const source of sources) {
        const li=document.createElement('li'); const identity=document.createElement('span'); identity.textContent=String(source.source_id||'');li.append(identity);
        const revision=source.revision_sha256||source.revision;
        if (revision) {const line=document.createElement('span');line.className='source-revision';line.textContent=t('revision')+': '+String(revision);li.append(line);}
        if (source.span_start!=null&&source.span_end!=null) {const span=document.createElement('span');span.textContent=source.span_start+'–'+source.span_end+' · raw_markdown_utf8';li.append(span);}
        list.append(li);
      }
      dd.append(list);$('metadata').append(dt,dd);
    }
    if (provenance.epistemic_state) {
      const states=lang==='en'?{proposed:'Proposed',verified:'Verified',conflicted:'Conflicted',superseded:'Superseded'}:{proposed:'Propuesto',verified:'Verificado',conflicted:'En conflicto',superseded:'Sustituido'};
      metadataLine(lang==='en'?'Evidence status':'Estado de evidencia',states[provenance.epistemic_state]||provenance.epistemic_state);
    }
  }
  function renderInspector(detail) {
    const node = state.selected; if (!node) return;
    $('inspect-content').hidden = false; $('inspect-empty').hidden = true; $('node-title').textContent = node.label || node.id; $('metadata').replaceChildren();
    const source = detail?.source || sourceOf(node);
    metadataLine(t('projectLabel'),node.project_label||projectName(node.project_id||state.project));
    if(node.memberships?.length>1)metadataLine(lang==='en'?'Authorized index/source memberships':'Pertenencias autorizadas de índice/origen',[...new Set(node.memberships.map(m=>m.project_label))].join(', ')+' · '+node.memberships.length);
    if(node.native_chunk_id)metadataLine('Native chunk ID',node.native_chunk_id);
    metadataLine(t('nodeType'),kindName(node.kind)); metadataLine(t('content'),t(detail?.content_kind || node.content_kind || 'none'));
    metadataLine(t('source'),[source.root_id,source.relative_path].filter(Boolean).join(' / ')); metadataLine(t('revision'),detail?.revision || source.revision); metadataLine(t('id'),node.id);
    const origin=node.originSelection||detail?.node?.origin||node.origin;
    if(origin?.relative_path)metadataLine(lang==='en'?'Original / indexed path':'Ruta original / indexada',origin.relative_path);
    if(origin?.origins?.length)metadataLine(t('provenance'),origin.origins.map(o=>[o.original_root,o.relative_path,o.revision||o.expected_sha256].filter(Boolean).join(' / ')).join('\n'));
    const revisionState=detail?.node?.revision_state ?? node.revision_state;
    metadataLine(lang==='en'?'Revision status':'Estado de revisión',revisionState==='current'?t('current'):revisionState==='historical'?t('historical'):(lang==='en'?'Unknown':'Desconocido'));
    if (node.chunk_id && node.chunk_id !== node.id) metadataLine(t('chunk'),node.chunk_id);
    metadataLine(t('offsetBasis'),detail?.offset_basis || source.offset_basis); if (source.start_byte != null) metadataLine(t('offsets'),source.start_byte + '–' + source.end_byte);
    const tierCell=metadataLine(t('tier'),'');const tierBadge=document.createElement('span');tierBadge.className='tier '+({HOT:'hot',WARM:'warm',COLD:'cold'}[node.tier]||'neutral');tierBadge.textContent=node.tier==='unclassified'?t('unclassified'):node.tier||t('unclassified');tierCell.append(tierBadge); metadataLine(t('tierOrigin'),node.tier_origin); metadataLine(t('pin'),detail?.pinned == null ? null : detail.pinned ? t('yes') : t('no'));
    provenanceLines(detail?.provenance); if (detail?.total_bytes != null) metadataLine(t('bytes'),detail.total_bytes);
    if (state.data?.projection?.model_identity) metadataLine(t('model'),state.data.projection.model_identity);
    $('read-source').disabled = node.kind === 'source_revision';
    $('read-chunk').textContent = node.kind === 'chunk' ? t('readChunk') : t('content');
    $('focus').disabled = false;
    renderRelations();
  }
  function renderRelations() {
    $('relations').replaceChildren(); const id = state.selected?.id; if (!id) return;
    const relations = state.edges.filter(edge => edge.source === id || edge.target === id);
    for (const edge of relations.slice(0,100)) {
      const target = state.nodeMap.get(edge.source === id ? edge.target : edge.source);
      const li = document.createElement('li'); li.className = 'relation';
      if (target) { const button = document.createElement('button'); button.textContent = target.label || target.id; button.addEventListener('click',() => selectNode(target)); li.append(button); }
      const description = document.createElement('span'); const kind = ({contains:'contains',explicit:'explicit',semantic_similarity:'similarity',derived_from:'derived'})[edge.kind] || edge.kind;
      description.textContent = t(kind) + ' · ' + (edge.provenance_scope || '') + (edge.original_link_kind ? ' · ' + edge.original_link_kind : '') + (edge.score == null ? '' : ' · ' + t('score') + ' ' + Number(edge.score).toFixed(4)) + (edge.model_identity ? ' · ' + edge.model_identity : '') + ' · ' + (edge.project_ids||[edge.project_id]).map(projectName).join(', '); li.append(description); $('relations').append(li);
    }
    if (!relations.length) { const li = document.createElement('li'); li.textContent = t('noRelations'); $('relations').append(li); }
  }
  function selectNode(node,member=null) {
    member=member||folderMember(node);
    if(member)node={...node,project_id:member.project_id,project_label:member.project_label,native_id:member.native_id,source:member.source,originSelection:member.originSelection};
    $('graph-view').classList.remove('inspector-collapsed');$('toggle-inspector').setAttribute('aria-expanded','true');state.lineKey=null;state.dirty=true;state.detailRequest++; state.detailController?.abort(); state.selected = node; state.detail = null; state.sourceMode = false;
    $('node-text').textContent = t('loadingDetail'); $('text-kind').textContent = t(node.content_kind || 'content'); $('more-text').hidden = true;
    renderInspector(null); renderList(); draw(); loadDetail(0,false);
  }
  async function loadDetail(offset = 0, source = false) {
    if (!state.selected || !state.data) return;
    state.detailController?.abort(); state.detailController = new AbortController(); const request = ++state.detailRequest;
    const node = state.selected; const project = state.project; state.sourceMode = source;
    const owner=node.project_id||project; const snapshot=state.data.snapshots?.[owner]?.snapshot_id||state.data.snapshot_id;
    const params = new URLSearchParams({snapshot_id:snapshot,offset:String(offset),limit:'16384'}); if (source) params.set('source','1');
    $('node-text').textContent = t('loadingDetail'); $('more-text').hidden = true;
    try {
      const detail = await api(projectEndpoint(owner,'nodes/' + encodeURIComponent(node.native_id||node.id)) + '?' + params,state.detailController.signal);
      if (request !== state.detailRequest || project !== state.project || node.id !== state.selected?.id) return;
      state.detail = detail; renderInspector(detail);
      $('text-kind').textContent = source ? t('rawSource') : t(detail.content_kind || node.content_kind || 'content');
      const text = source && detail.raw_markdown != null ? detail.raw_markdown : detail.text;
      $('node-text').textContent = text == null || text === '' ? t('noText') : String(text);
      if (source) metadataLine(t('offsetBasis'),'raw_markdown_utf8 · ' + t('sourceHint'));
      $('more-text').hidden = detail.next_offset == null; $('node-text').scrollTop = 0;
    } catch (error) {
      if (request !== state.detailRequest || error.name === 'AbortError') return;
      $('node-text').textContent = ''; showError(error);
    }
  }
  function setExpanded(value){expanded=value;document.body.classList.toggle('graph-expanded',value);text('expand-graph',t(value?'collapseGraph':'expandGraph'));$('expand-graph').setAttribute('aria-expanded',String(value));$('graph-view').setAttribute('role',value?'dialog':'region');if(value)$('graph-view').setAttribute('aria-modal','true');else $('graph-view').removeAttribute('aria-modal');for(const el of document.querySelectorAll('header,aside.controls,footer'))el.inert=value;requestAnimationFrame(()=>{resize();fitGraph();$('expand-graph').focus({preventScroll:true});});}
  function closeAndReturn(){$('graph-view').classList.add('inspector-collapsed');$('toggle-inspector').setAttribute('aria-expanded','false');clearInspector();renderList();canvas.focus({preventScroll:true});}
  $('zoom-in').addEventListener('click',()=>zoom(1.25));$('zoom-out').addEventListener('click',()=>zoom(.8));$('fit').addEventListener('click',fitGraph);
  $('expand-graph').addEventListener('click',()=>setExpanded(!expanded));$('close-inspector').addEventListener('click',closeAndReturn);
  $('focus').addEventListener('click',()=>{if(!state.selected)return;state.focused=new Set([state.selected.id]);for(const e of state.edges)if(e.source===state.selected.id||e.target===state.selected.id){state.focused.add(e.source);state.focused.add(e.target);}applyFilters();fitGraph();$('unfocus').disabled=false;});
  $('unfocus').addEventListener('click',()=>{state.focused=null;$('unfocus').disabled=true;applyFilters();fitGraph();});
  $('all-scope').addEventListener('click',()=>{if(state.project===ALL_PROJECTS)$('projects').value=ALL_PROJECTS;resetFilters();});
  $('projects').addEventListener('change',changeProject);$('refresh').addEventListener('click',loadGraph);$('reset').addEventListener('click',()=>resetFilters());
  for(const id of ['tier-filter','revision-filter','isolation-filter'])$(id).addEventListener('change',()=>applyFilters());
  for(const input of document.querySelectorAll('fieldset input'))input.addEventListener('change',()=>applyFilters());
  for(const id of ['edge-opacity','edge-detail','edge-zoom'])$(id).addEventListener('input',draw);
  $('color-mode').addEventListener('change',()=>{document.body.dataset.colorMode=$('color-mode').value;$('tier-counts').hidden=$('color-mode').value!=='tier';state.dirty=true;draw();});
  let searchTimer;$('search').addEventListener('input',()=>{clearTimeout(searchTimer);searchTimer=setTimeout(()=>applyFilters(),180);});
  $('list-next').addEventListener('click',()=>{state.listPage++;renderList();});$('list-previous').addEventListener('click',()=>{state.listPage--;renderList();});
  $('node-list').addEventListener('keydown',event=>{const buttons=[...$('node-list').querySelectorAll('button')],index=buttons.indexOf(document.activeElement);if(index<0)return;let next=index;if(event.key==='ArrowDown')next=Math.min(index+1,buttons.length-1);else if(event.key==='ArrowUp')next=Math.max(index-1,0);else if(event.key==='Home')next=0;else if(event.key==='End')next=buttons.length-1;else return;event.preventDefault();buttons[next]?.focus();});
  $('read-chunk').addEventListener('click',()=>loadDetail(0,false));$('read-source').addEventListener('click',()=>loadDetail(0,true));$('more-text').addEventListener('click',()=>{if(state.detail?.next_offset!=null)loadDetail(state.detail.next_offset,state.sourceMode);});
  $('copy-ref').addEventListener('click',async()=>{const n=state.selected;if(!n)return;const s=sourceOf(n),ref=[n.project_id,s.root_id,s.relative_path,s.revision,n.native_chunk_id||n.chunk_id||n.native_id].join(' · ');try{await navigator.clipboard.writeText(ref);message('notice',t('copied'));}catch(_){message('notice',t('copyFailed'));}});
  $('language').addEventListener('change',()=>{lang=$('language').value;translate();});
  document.addEventListener('keydown',event=>{if(event.key==='Escape'){if(expanded)setExpanded(false);else closeAndReturn();hideTooltip();}else if(expanded&&event.key==='Tab'){const items=[...$('graph-view').querySelectorAll('button,select,input,summary,[tabindex]')].filter(e=>!e.disabled&&e.getClientRects().length),first=items[0],last=items.at(-1);if(event.shiftKey&&document.activeElement===first){event.preventDefault();last?.focus();}else if(!event.shiftKey&&document.activeElement===last){event.preventDefault();first?.focus();}}});
  async function pollStatus(){
    if(!state.data||state.polling||!token)return;state.polling=true;const request=state.request;
    try{for(const [id,scope]of Object.entries(state.data.snapshots)){
      const status=await api(projectEndpoint(id,'status'));if(request!==state.request)return;
      if(status.policy_fingerprint!==scope.policy_fingerprint||status.changed||status.snapshot_id!==scope.snapshot_id){clearGraph();message('error',status.reason==='SOURCE_CHANGED'?t('sourceMismatch'):t('revoked'));return;}
    }}catch(error){if(request===state.request){clearGraph();showError(error);if(!['ROOT_REVOKED','UNAUTHORIZED','PROFILE_NOT_FOUND','SNAPSHOT_EXPIRED'].includes(errorCode(error)))message('error',t('statusUnknown'));}}finally{state.polling=false;}
  }
  let treeScrollFrame=0;$('scope-tree').addEventListener('scroll',()=>{if(!treeScrollFrame)treeScrollFrame=requestAnimationFrame(()=>{treeScrollFrame=0;renderTree(false);});},{passive:true});
  $('back-nucleus').addEventListener('click',goBack);
  $('toggle-inspector').addEventListener('click',()=>{const collapsed=$('graph-view').classList.toggle('inspector-collapsed');$('toggle-inspector').setAttribute('aria-expanded',String(!collapsed));requestAnimationFrame(resize);});
  for(const id of ['nucleus-separation','nucleus-density'])$(id).addEventListener('input',()=>{layout();applyFilters(false);fitGraph();});
  $('minimap').addEventListener('click',event=>{const m=state.miniTransform;if(!m)return;const r=$('minimap').getBoundingClientRect(),x=(event.clientX-r.left-m.ox)/m.scale+m.b.minX,y=(event.clientY-r.top-m.oy)/m.scale+m.b.minY;state.panX=canvas.clientWidth/2-x*state.scale;state.panY=canvas.clientHeight/2-y*state.scale;draw();});
  $('minimap').addEventListener('keydown',event=>{if(event.key==='Enter'){fitGraph();event.preventDefault();}else if(['ArrowLeft','ArrowRight','ArrowUp','ArrowDown'].includes(event.key)){state.panX+=event.key==='ArrowLeft'?60:event.key==='ArrowRight'?-60:0;state.panY+=event.key==='ArrowUp'?60:event.key==='ArrowDown'?-60:0;draw();event.preventDefault();}});
  const nodeDiagnostic=id=>{let n=state.nodeMap.get(id);const p=displayPosition(id);if(!n)return null;const member=folderMember(n);if(member)n={...n,project_id:member.project_id,native_id:member.native_id,source:member.source,originSelection:member.originSelection};return {id:n.id,native_id:n.native_id,native_chunk_id:n.native_chunk_id,project_id:n.project_id,nucleus_id:n.nucleus_id,source:{...n.source},origin:n.origin?JSON.parse(JSON.stringify(n.origin)):null,position:p?{...p}:null,memberships:n.memberships.map(m=>({...m,scopeKeys:undefined}))};};
  Object.defineProperty(window,'__VRG_VIEW__',{value:Object.freeze({
    snapshot(options={}){return {loading:state.loading,loadedNodes:state.nodes.length,loadedMemberships:state.loadedMemberships,visibleNodes:state.visible.length,gpuResidentNodes:state.gpuResident||0,renderedNodes:state.rendered,inViewportNodes:state.inViewport||0,lod:lodLevel(),scale:state.scale,panX:state.panX,panY:state.panY,positionHash:state.positionHash,selectedId:state.selected?.id||null,focusedProjectId:state.navigation.projectId,focusedFolderKey:state.navigation.folderKey,overlayLabels:state.overlayLabels.map(x=>({...x})),dataCoverage:[...state.projectStats.values()].map(x=>({...x})),projects:(state.nuclei?.projects||[]).map(g=>({id:g.id,label:g.label,x:g.x,y:g.y,radius:g.radius,color:{...g.color},count:g.count,nodeCount:g.nodeIds.length,folders:g.folders.map(f=>({key:f.key,label:f.label,path:f.path,parentKey:f.parentKey,depth:f.depth,x:f.x,y:f.y,radius:f.radius,count:f.count,anchorCount:f.anchorNodeIds.length}))})),...(options.positions?{positions:[...state.positions].map(([id,p])=>({id,...p}))}:{})};},
    node:nodeDiagnostic,
    folderNodes(projectId,key){const g=state.nuclei?.projects.find(g=>g.id===projectId);return [...(g?.folders.find(f=>f.key===key)?.nodeIds||g?.nodeIds||[])];}
  }),writable:false,configurable:false});
  document.body.dataset.colorMode='project';
  initGpu();if(!gl)message('error',t('webgl'));new ResizeObserver(resize).observe(canvas);setInterval(pollStatus,5000);
  addEventListener('pagehide',()=>{token='';clearGraph();});translate();boot();
})();
