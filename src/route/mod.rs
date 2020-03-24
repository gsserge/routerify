use crate::prelude::*;
use crate::router::Router;
use crate::{PathParams, RequestData};
use futures::future::{BoxFuture, FutureExt};
use hyper::upgrade::Upgraded;
use hyper::{Body, Method, Request, Response};
use regex::Regex;
use std::future::Future;
use std::pin::Pin;

mod regex_generator;

type BoxedNormalRouteHandler = Box<dyn Fn(Request<Body>) -> BoxedNormalRouteResponse + Send + Sync + 'static>;
type BoxedNormalRouteResponse = Box<dyn Future<Output = crate::Result<Response<Body>>> + Send + Sync + 'static>;
type BoxedWsRouteHandler = Box<dyn Fn(Upgraded, RequestData) -> BoxedWsRouteResponse + Send + Sync + 'static>;
type BoxedWsRouteResponse = Box<dyn Future<Output = crate::Result<()>> + Send + Sync + 'static>;

pub struct Route {
  path: String,
  regex: Regex,
  path_params: Vec<String>,
  inner: Inner,
}

enum Inner {
  Normal(Vec<Method>, BoxedNormalRouteHandler),
  Router(&'static Router),
  WS(BoxedWsRouteHandler),
}

impl Route {
  pub fn with_normal<P, H, R>(path: P, methods: Vec<Method>, handler: H) -> crate::Result<Route>
  where
    P: Into<String>,
    H: Fn(Request<Body>) -> R + Send + Sync + 'static,
    R: Future<Output = crate::Result<Response<Body>>> + Send + Sync + 'static,
  {
    let path = path.into();
    let (re, params) = Route::gen_exact_match_regex(path.as_str())?;

    let handler: BoxedNormalRouteHandler = Box::new(move |req: Request<Body>| Box::new(handler(req)));

    Ok(Route {
      path,
      regex: re,
      path_params: params,
      inner: Inner::Normal(methods, handler),
    })
  }

  pub fn with_router<P>(path: P, router: &'static Router) -> crate::Result<Route>
  where
    P: Into<String>,
  {
    let path = path.into();
    let (re, params) = Self::gen_prefix_match_regex(path.as_str())?;

    Ok(Route {
      path,
      regex: re,
      path_params: params,
      inner: Inner::Router(router),
    })
  }

  pub fn with_ws<P, H, R>(path: P, handler: H) -> crate::Result<Route>
  where
    P: Into<String>,
    H: Fn(Upgraded, RequestData) -> R + Send + Sync + 'static,
    R: Future<Output = crate::Result<()>> + Send + Sync + 'static,
  {
    let path = path.into();
    let (re, params) = Self::gen_exact_match_regex(path.as_str())?;

    let handler: BoxedWsRouteHandler =
      Box::new(move |upgraded: Upgraded, req_data: RequestData| Box::new(handler(upgraded, req_data)));
    Ok(Route {
      path,
      regex: re,
      path_params: params,
      inner: Inner::WS(handler),
    })
  }

  fn gen_exact_match_regex(path: &str) -> crate::Result<(Regex, Vec<String>)> {
    regex_generator::generate_exact_match_regex(path)
      .context("Could not create an exact match regex for the route path")
  }

  fn gen_prefix_match_regex(path: &str) -> crate::Result<(Regex, Vec<String>)> {
    regex_generator::generate_prefix_match_regex(path)
      .context("Could not create a prefix match regex for the route path")
  }

  pub fn is_match(&self, target_path: &str, method: &Method) -> bool {
    match self.inner {
      Inner::Normal(ref methods, _) => {
        if methods.len() > 0 {
          self.regex.is_match(target_path) && methods.contains(method)
        } else {
          self.regex.is_match(target_path)
        }
      }
      Inner::Router(_) => self.regex.is_match(target_path),
      Inner::WS(_) => self.regex.is_match(target_path),
    }
  }

  pub fn path(&self) -> &String {
    &self.path
  }

  pub async fn process(&self, target_path: &str, req: Request<Body>) -> crate::Result<Response<Body>> {
    match self.inner {
      Inner::Normal(_, ref handler) => self.process_normal_route(target_path, req, handler).await,
      Inner::Router(router) => self.process_router_route(target_path, req, router).await,
      Inner::WS(ref handler) => self.process_ws_route(target_path, req, handler).await,
    }
  }

  async fn process_normal_route(
    &self,
    target_path: &str,
    mut req: Request<Body>,
    handler: &BoxedNormalRouteHandler,
  ) -> crate::Result<Response<Body>> {
    self.push_req_data(target_path, &mut req);
    Pin::from(handler(req)).await
  }

  fn process_router_route(
    &self,
    target_path: &str,
    mut req: Request<Body>,
    router: &'static Router,
  ) -> BoxFuture<'static, crate::Result<Response<Body>>> {
    self.push_req_data(target_path, &mut req);
    let target_path: String = self.regex.replace(target_path, "").into();
    async move { router.process(target_path.as_str(), req).await }.boxed()
  }

  async fn process_ws_route(
    &self,
    target_path: &str,
    _req: Request<Body>,
    _handler: &BoxedWsRouteHandler,
  ) -> crate::Result<Response<Body>> {
    let _req_data = self.generate_req_data(target_path);
    todo!("Websocket support is not yet added");
  }

  fn push_req_data(&self, target_path: &str, req: &mut Request<Body>) {
    self.update_req_data(req, self.generate_req_data(target_path));
  }

  fn update_req_data(&self, req: &mut Request<Body>, req_data: RequestData) {
    let ext = req.extensions_mut();

    if let Some(existing_req_data) = ext.get_mut::<RequestData>() {
      existing_req_data.extend(req_data)
    } else {
      ext.insert(req_data);
    }
  }

  fn generate_req_data(&self, target_path: &str) -> RequestData {
    let path_params_list = &self.path_params;
    let ln = path_params_list.len();

    let mut path_params = PathParams::with_capacity(ln);

    if ln > 0 {
      if let Some(caps) = self.regex.captures(target_path) {
        for idx in 0..ln {
          if let Some(g) = caps.get(idx + 1) {
            path_params.set(path_params_list[idx].clone(), String::from(g.as_str()));
          }
        }
      }
    }

    RequestData::new(path_params)
  }
}