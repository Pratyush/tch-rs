//! Variable stores.
use super::Init;
use crate::tensor::Tensor;
use crate::{Device, Kind};
use failure::Fallible;
use std::collections::HashMap;
use std::ops::Div;
use std::sync::Mutex;

/// The separator is used to separate path elements in the tensor names.
const SEP: char = '|';

// When the variable store is frozen, trainable still is set to tree,
// however the tensor is not set to require gradients.
#[derive(Debug)]
struct Variable {
    tensor: Tensor,
    trainable: bool,
}

/// A VarStore is used to store variables used by one or multiple layers.
/// It specifies a single device where all variables are stored.
#[derive(Debug)]
pub struct VarStore {
    variables: Mutex<HashMap<String, Variable>>,
    device: Device,
}

/// A variable store with an associated path for variables naming.
#[derive(Debug)]
pub struct Path<'a> {
    path: Vec<String>,
    var_store: &'a VarStore,
}

impl VarStore {
    /// Creates a new var-store located on the specified device.
    pub fn new(device: Device) -> VarStore {
        VarStore {
            variables: Mutex::new(HashMap::new()),
            device,
        }
    }

    /// Gets the device for this var-store.
    pub fn device(&self) -> Device {
        self.device
    }

    /// Returns all the trainable variables for this var-store.
    pub fn trainable_variables(&self) -> Vec<Tensor> {
        let variables = self.variables.lock().unwrap();
        variables
            .values()
            .filter_map(|v| {
                if v.trainable {
                    Some(v.tensor.shallow_clone())
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn root(&self) -> Path {
        Path {
            path: vec![],
            var_store: self,
        }
    }

    /// Saves the var-store variable values to a file.
    pub fn save<T: AsRef<std::path::Path>>(&self, path: T) -> Fallible<()> {
        let variables = self.variables.lock().unwrap();
        let named_tensors = variables
            .iter()
            .map(|(x, y)| (&x[..], &y.tensor))
            .collect::<Vec<_>>();
        Tensor::save_multi(named_tensors.as_slice(), path)
    }

    /// Loads the var-store variable values from a file.
    pub fn load<T: AsRef<std::path::Path>>(&mut self, path: T) -> Fallible<()> {
        let named_tensors = Tensor::load_multi(&path)?;
        let named_tensors: HashMap<_, _> = named_tensors.into_iter().collect();
        let mut variables = self.variables.lock().unwrap();
        for (name, var) in variables.iter_mut() {
            match named_tensors.get(name) {
                Some(src) => crate::no_grad(|| {
                    var.tensor
                        .f_copy_(src)
                        .map_err(|e| format_err!("{}: {}", name, e))
                })?,
                None => return Err(format_err!("cannot find {} in {:?}", name, path.as_ref())),
            }
        }
        Ok(())
    }

    pub fn freeze(&mut self) {
        let variables = self.variables.lock().unwrap();
        for variable in variables.values() {
            if variable.trainable {
                let _v = variable.tensor.set_requires_grad(false);
            }
        }
    }

    pub fn unfreeze(&mut self) {
        let variables = self.variables.lock().unwrap();
        for variable in variables.values() {
            if variable.trainable {
                let _v = variable.tensor.set_requires_grad(true);
            }
        }
    }
}

impl<'a> Path<'a> {
    pub fn sub(&'a self, s: &str) -> Path<'a> {
        if s.chars().any(|x| x == SEP) {
            panic!("sub name cannot contain {} {}", SEP, s);
        }
        let mut path = self.path.clone();
        path.push(s.to_owned());
        Path {
            path,
            var_store: self.var_store,
        }
    }

    pub fn device(&self) -> Device {
        self.var_store.device
    }

    fn path(&self, name: &str) -> String {
        if name.chars().any(|x| x == SEP) {
            panic!("variable name cannot contain {} {}", SEP, name);
        }
        if self.path.is_empty() {
            name.to_string()
        } else {
            format!("{}{}{}", self.path.join(&SEP.to_string()), SEP, name)
        }
    }

    fn add(&self, name: &str, tensor: Tensor, trainable: bool) -> Tensor {
        let path = self.path(name);
        let mut variables = self.var_store.variables.lock().unwrap();
        let path = if variables.contains_key(&path) {
            format!("{}__{}", path, variables.len())
        } else {
            path
        };
        let tensor = if trainable {
            tensor.set_requires_grad(true)
        } else {
            tensor
        };
        let var = Variable {
            tensor: tensor.shallow_clone(),
            trainable,
        };
        variables.insert(path, var);
        tensor
    }

    pub fn zeros_no_train(&self, name: &str, dims: &[i64]) -> Tensor {
        let z = Tensor::zeros(dims, (Kind::Float, self.device()));
        self.add(name, z, false)
    }

    pub fn ones_no_train(&self, name: &str, dims: &[i64]) -> Tensor {
        let o = Tensor::ones(dims, (Kind::Float, self.device()));
        self.add(name, o, false)
    }

    pub fn var(&self, name: &str, dims: &[i64], init: Init) -> Tensor {
        let v = super::init(init, dims, self.device());
        self.add(name, v, true)
    }

    pub fn zeros(&self, name: &str, dims: &[i64]) -> Tensor {
        self.var(name, dims, Init::Const(0.))
    }

    pub fn ones(&self, name: &str, dims: &[i64]) -> Tensor {
        self.var(name, dims, Init::Const(1.))
    }

    pub fn randn_standard(&self, name: &str, dims: &[i64]) -> Tensor {
        let init = Init::Randn {
            mean: 0.,
            stdev: 1.,
        };
        self.var(name, dims, init)
    }

    pub fn randn(&self, name: &str, dims: &[i64], mean: f64, stdev: f64) -> Tensor {
        self.var(name, dims, Init::Randn { mean, stdev })
    }

    pub fn uniform(&self, name: &str, dims: &[i64], lo: f64, up: f64) -> Tensor {
        self.var(name, dims, Init::Uniform { lo, up })
    }

    pub fn kaiming_uniform(&self, name: &str, dims: &[i64]) -> Tensor {
        self.var(name, dims, Init::KaimingUniform)
    }

    pub fn var_copy(&self, name: &str, t: &Tensor) -> Tensor {
        let mut v = self.zeros(name, &t.size());
        crate::no_grad(|| v.copy_(&t));
        v
    }
}

impl<'a> Div<&str> for &'a mut Path<'a> {
    type Output = Path<'a>;

    fn div(self, rhs: &str) -> Self::Output {
        self.sub(&rhs)
    }
}

impl<'a> Div<&str> for &'a Path<'a> {
    type Output = Path<'a>;

    fn div(self, rhs: &str) -> Self::Output {
        self.sub(&rhs)
    }
}
